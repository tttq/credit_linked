//! 华为 CodeArts 适配器（Agent/Desktop）。
//! STS token（DPoP ES256 p1363）+ SDK-HMAC-SHA256 请求签名 + /v1/ops/{delivery,claim,confirm}。
//! 幂等键 SHA256("{uid}:{day}:{campaignId}")。

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use p256::{SecretKey, ecdsa::signature::Signer, ecdsa::SigningKey};

use crate::adapters::*;
use crate::adapters::trae::urlencoding_lite;
use crate::error::PlatformError;
use crate::net::http::business;
use reqwest::Method;

pub const STS_BASE: &str = "https://sts.cn-north-4.myhuaweicloud.com";
pub const API_BASE: &str = "https://snap-access.cn-north-4.myhuaweicloud.com";

pub struct CodeArtsAdapter { pub desktop: bool }

// ===== DPoP =====

pub fn dpop_header(private_jwk: &Value, htu: &str) -> Result<String, PlatformError> {
    let sk = SigningKey::from(JwkDec::from_jwk(private_jwk)?);
    let header = B64URL.encode(json!({
        "typ": "dpop+jwt", "alg": "ES256", "jwk": public_jwk_of(&sk),
    }).to_string());
    let payload = B64URL.encode(json!({
        "htu": htu, "htm": "POST", "iat": now_secs(), "jti": uuid::Uuid::new_v4().to_string(),
    }).to_string());
    let msg = format!("{header}.{payload}");
    let sig: p256::ecdsa::Signature = sk.sign(msg.as_bytes()); // p1363: r||s
    Ok(format!("{msg}.{}", B64URL.encode(sig.to_bytes())))
}

struct JwkDec;
impl JwkDec {
    fn from_jwk(jwk: &Value) -> Result<SecretKey, PlatformError> {
        let d = jwk.get("d").and_then(|x| x.as_str()).ok_or_else(|| PlatformError::Format("jwk missing d".into()))?;
        let d_bytes = b64url_decode(d)?;
        let sk = SecretKey::from_slice(&d_bytes).map_err(|e| PlatformError::Format(format!("bad jwk d: {e}")))?;
        Ok(sk)
    }
}

pub fn public_jwk_of(sk: &SigningKey) -> Value {
    let vk = sk.verifying_key();
    let enc = vk.to_encoded_point(false); // uncompressed: 04||X||Y
    let bytes = enc.as_bytes();
    let x = b64url_encode(&bytes[1..33]);
    let y = b64url_encode(&bytes[33..65]);
    json!({ "kty": "EC", "crv": "P-256", "x": x, "y": y })
}

pub fn b64url_decode(s: &str) -> Result<Vec<u8>, PlatformError> {
    B64URL.decode(s).map_err(|e| PlatformError::Format(format!("b64url: {e}")))
}
pub fn b64url_encode(b: &[u8]) -> String { B64URL.encode(b) }

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

// ===== STS token（AK/SK/securityToken） =====

/// POST /v1/oauth2/tokens（grant_type=authorization_code 或 refresh_token）+ DPoP
pub async fn sts_token(body_pairs: &[(&str, &str)], private_jwk: &Value) -> Result<Value, PlatformError> {
    let url = format!("{STS_BASE}/v1/oauth2/tokens");
    let dpop = dpop_header(private_jwk, &url)?;
    let body = body_pairs.iter().map(|(k, v)| format!("{}={}", crate::adapters::trae::urlencoding_lite(k), crate::adapters::trae::urlencoding_lite(v))).collect::<Vec<_>>().join("&");
    let _hs = vec![
        ("Content-Type", "application/x-www-form-urlencoded".to_string()),
        ("DPoP", dpop.clone()),
    ];
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(std::time::Duration::from_secs(15)).build().unwrap();
    let resp = client.post(&url).header("Content-Type", "application/x-www-form-urlencoded").header("DPoP", dpop).body(body)
        .send().await.map_err(|e| PlatformError::Network(e.to_string()))?;
    match resp.status().as_u16() { 401 => return Err(PlatformError::Auth), 429 => return Err(PlatformError::Rate), _ => {} }
    let v: Value = resp.json().await.map_err(|e| PlatformError::Format(e.to_string()))?;
    Ok(v)
}
// ===== SDK-HMAC-SHA256 请求签名 =====

pub fn decode_percent(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 1 && i + 2 <= b.len() - 1 + 1 && i + 2 < b.len() {
            let h = (b[i + 1] as char).to_digit(16).unwrap_or(99) as u32;
            let l = (b[i + 2] as char).to_digit(16).unwrap_or(99) as u32;
            if h < 16 && l < 16 { out.push(((h << 4) | l) as u8); i += 3; continue; }
        }
        out.push(b[i]); i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn sign_codearts(
    ak: &str, sk: &str, method: &str, host: &str, url_path: &str, query: &str,
    headers: &[(String, String)], body: &str, security_token: Option<&str>,
) -> (String, Vec<(String, String)>) {
    // 参与签名的 header：content-type / host / x-sdk-date [+ x-security-token] [+ 业务头]
    let mut sign_headers: Vec<(String, String)> = headers.to_vec();
    let x_sdk_date = gmt_iso8601();
    sign_headers.push(("host".into(), host.to_string()));
    if !sign_headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("x-sdk-date")) {
        sign_headers.push(("x-sdk-date".into(), x_sdk_date.clone()));
    }
    if let Some(st) = security_token {
        sign_headers.retain(|(k, _)| !k.eq_ignore_ascii_case("x-security-token"));
        sign_headers.push(("x-security-token".into(), st.to_string()));
    }
    // canonical: 小写化 header 名并排序
    sign_headers.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    let canonical_headers: String = sign_headers.iter()
        .map(|(k, v)| format!("{}:{}\n", k.to_lowercase(), v.trim()))
        .collect();
    let signed_header_names: String = sign_headers.iter().map(|(k, _)| k.to_lowercase()).collect::<Vec<_>>().join(";");
    let body_hash = hex_lower(&Sha256::digest(body.as_bytes()));
    let canonical_query = canonical_query_str(query);
    let mut uri = url_path.split('/').map(|seg| {
        // vendor: encode(decodeURIComponent(seg)) per segment
        urlencoding_lite(&decode_percent(seg))
    }).collect::<Vec<_>>().join("/");
    if !uri.ends_with('/') { uri.push('/'); }
    let canonical = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method.to_uppercase(), uri, canonical_query, canonical_headers, signed_header_names, body_hash
    );
    let hashed = hex_lower(&Sha256::digest(canonical.as_bytes()));
    let mut mac = Hmac::<Sha256>::new_from_slice(sk.as_bytes()).unwrap();
    mac.update(format!("SDK-HMAC-SHA256\n{x_sdk_date}\n{hashed}").as_bytes());
    let sig = hex_lower(&mac.finalize().into_bytes());
    let auth = format!("SDK-HMAC-SHA256 Access={ak}, SignedHeaders={signed_header_names}, Signature={sig}");
    (auth, sign_headers)
}

fn canonical_query_str(query: &str) -> String {
    // 排序 + 编码（原版使用严格 URL 编码规则；对常见 query 对齐）
    let mut pairs: Vec<(String, String)> = query.split('&').filter(|s| !s.is_empty()).map(|p| {
        match p.split_once('=') { Some((k, v)) => (k.to_string(), v.to_string()), None => (p.to_string(), String::new()) }
    }).collect();
    pairs.sort();
    pairs.iter().map(|(k, v)| format!("{}={}", crate::adapters::trae::urlencoding_lite(k), crate::adapters::trae::urlencoding_lite(v))).collect::<Vec<_>>().join("&")
}

fn gmt_iso8601() -> String {
    // yyyymmddTHHMMSSZ（压缩 ISO8601，UTC）
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let days = secs / 86400;
    let rem = secs % 86400;
    let (y, mo, d) = crate::adapters::civil_from_days(days);
    format!("{y:04}{mo:02}{d:02}T{:02}{:02}{:02}Z", rem / 3600, (rem % 3600) / 60, rem % 60)
}

pub fn hex_lower(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ===== 业务请求 =====

fn channel(desktop: bool) -> &'static str { if desktop { "DESKTOP" } else { "IDE" } }

pub async fn ops_request(c: &Cred, _desktop: bool, method: Method, path: &str, query: &str, body: Value, identity: bool) -> Result<Value, PlatformError> {
    let base = if identity { STS_BASE } else { API_BASE };
    let url = format!("{base}{path}");
    let ak = c.meta.get("access_key_id").and_then(|a| a.as_str()).unwrap_or_default();
    let sk = c.meta.get("secret_access_key").and_then(|a| a.as_str()).unwrap_or_default();
    let st = c.meta.get("security_token").and_then(|a| a.as_str());
    let body_str = if body.is_null() { String::new() } else { serde_json::to_string(&body).unwrap_or_default() };
    let mut base_headers: Vec<(String, String)> = vec![("content-type".into(), "application/json".into())];
    // 客户端真实头集（desktop 逆向）：X-Security-Token + Agent-Type + X-Language（+ x-snap-traceid）
    if let Some(tok) = st {
        if !tok.is_empty() {
            base_headers.push(("x-security-token".into(), tok.to_string()));
        }
    }
    base_headers.push(("agent-type".into(), "PromptCenter".into()));
    base_headers.push(("x-language".into(), "zh-cn".into()));
    // delivery/claim 等 GET 请求客户端带 trace id（uuid）且参与签名
    if !identity {
        base_headers.push(("x-snap-traceid".into(), uuid::Uuid::new_v4().to_string()));
    }
    let host_of_url = url.split("://").nth(1).unwrap_or("").split('/').next().unwrap_or("").to_string();
    let (auth, sign_headers) = sign_codearts(ak, sk, method.as_str(), &host_of_url, path, query, &base_headers, &body_str, st);
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(std::time::Duration::from_secs(15)).build().unwrap();
    let full = if query.is_empty() { url.clone() } else { format!("{url}?{query}") };
    let mut req = client.request(method, &full);
    for (k, v) in &sign_headers {
        req = req.header(k, v);
    }
    req = req.header("Authorization", auth);
    if !body_str.is_empty() {
        req = req.body(body_str);
    }
    let resp = req.send().await.map_err(|e| PlatformError::Network(e.to_string()))?;
    match resp.status().as_u16() { 401 => return Err(PlatformError::Auth), 429 => return Err(PlatformError::Rate), 403 => return Err(PlatformError::Forbidden), _ => {} }
    let v: Value = resp.json().await.map_err(|e| PlatformError::Format(e.to_string()))?;
    // HWS 网关常在 HTTP 200 里返回 error_code（如 APIG.0602 security token 过期）
    let ec = v.get("error_code").and_then(|e| e.as_str()).unwrap_or("");
    let em = v.get("error_msg").and_then(|e| e.as_str()).unwrap_or("");
    if !ec.is_empty() {
        if ec.contains("0602") || ec.contains("0603") || ec.contains("1806")
            || em.contains("token") && em.contains("expired") || em.contains("expired") {
            return Err(PlatformError::Auth);
        }
        return Err(PlatformError::Format(format!("CodeArts {ec}: {em}")));
    }
    business(v)
}

#[async_trait::async_trait]
impl Adapter for CodeArtsAdapter {
    fn provider(&self) -> &'static str { if self.desktop { "codearts-desktop" } else { "codearts" } }

    async fn verify(&self, c: &Cred) -> Result<Value, PlatformError> {
        // GET /v5/caller-identity（on STS）
        let v = ops_request(c, self.desktop, Method::GET, "/v5/caller-identity", "", Value::Null, true).await?;
        Ok(v)
    }

    async fn state(&self, c: &Cred) -> Result<CheckinState, PlatformError> {
        let q = format!("channel={}", channel(self.desktop));
        let v = ops_request(c, self.desktop, Method::GET, "/v1/ops/delivery", &q, Value::Null, false).await?;
        let items = v.pointer("/data/items").or_else(|| v.get("items")).and_then(|i| i.as_array()).cloned().unwrap_or_default();
        // 客户端真实判定（desktop 逆向）：campaignId 非空 && claimable !== false；
        // 未领 = status == ELIGIBLE；已入账 = status ∈ {CLAIMED, CONFIRMED, CONSUMED}。
        // 不做 triggerMode/时间窗过滤（desktop 无这些字段；USER_LOGIN 只是旧映射，
        // 桌面签到活动 type = DAILY_CLAIM）。
        let daily: Vec<Value> = items.into_iter().filter(|x| {
            let cid = match x.get("campaignId").or_else(|| x.get("campaign_id")) {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Number(n)) => n.to_string(),
                _ => String::new(),
            };
            if cid.is_empty() { return false; }
            x.get("claimable").and_then(|b| b.as_bool()) != Some(false)
        }).collect();
        let granted_status = |s: &str| ["CLAIMED", "CONFIRMED", "CONSUMED"].contains(&s);
        let checked = !daily.is_empty() && daily.iter().all(|x| granted_status(x.get("status").and_then(|t| t.as_str()).unwrap_or("")));
        let claimable = daily.iter().any(|x| x.get("status").and_then(|t| t.as_str()) == Some("ELIGIBLE"));
        Ok(CheckinState { checked, claimable, raw: json!({ "items": daily }), ..Default::default() })
    }

    async fn claim(&self, c: &Cred, st: &CheckinState) -> Result<Receipt, PlatformError> {
        if st.checked {
            return Ok(Receipt { status: "already".into(), amount: None, message: None });
        }
        let items = st.raw.get("items").and_then(|x| x.as_array()).cloned().unwrap_or_default();
        if items.is_empty() {
            return Ok(Receipt { status: "no-activity".into(), amount: None, message: None });
        }
        // 幂等键对齐客户端：claim_{campaignId}_{epoch_ms}（sha256 稳定键会被服务端幂等去重误伤重试）
        let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
        let mut amount: Option<f64> = None;
        for it in &items {
            let cid = match it.get("campaignId").or_else(|| it.get("campaign_id")) {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Number(n)) => n.to_string(),
                _ => String::new(),
            };
            if cid.is_empty() { continue; }
            let body = json!({
                "campaignId": cid,
                "channel": channel(self.desktop),
                "idempotentKey": format!("claim_{cid}_{now_ms}"),
            });
            let d = ops_request(c, self.desktop, Method::POST, "/v1/ops/claim", "", body, false).await?;
            // amount 桌面版从 delivery item 的 benefitAmount 取（claim 回执不带金额）
            let amt = it.get("benefitAmount").and_then(|a| a.as_f64())
                .or_else(|| d.pointer("/data/amount").and_then(|a| a.as_f64()))
                .or_else(|| d.pointer("/data/benefitAmount").and_then(|a| a.as_f64()))
                .or_else(|| d.get("amount").and_then(|a| a.as_f64()))
                .unwrap_or(0.0);
            amount = Some(amount.unwrap_or(0.0) + amt);
            // confirm：仅当 claim 回执带 id/userBenefitId 才发（与客户端一致；失败不阻塞）
            let bid = d.pointer("/data/id").or_else(|| d.get("id"))
                .or_else(|| d.pointer("/data/userBenefitId")).or_else(|| d.get("userBenefitId"))
                .cloned().unwrap_or(Value::Null);
            if !bid.is_null() {
                let confirm_body = if self.desktop {
                    json!({ "userBenefitId": bid })
                } else {
                    json!({ "campaignId": cid })
                };
                let _ = ops_request(c, self.desktop, Method::POST, "/v1/ops/confirm", "", confirm_body, false).await?;
            }
        }
        Ok(Receipt { status: "success".into(), amount, message: None })
    }

    async fn quota(&self, c: &Cred) -> Result<Quota, PlatformError> {
        let v = ops_request(c, self.desktop, Method::GET, "/snap-manager/v1/statistics/plugin", "", Value::Null, false).await?;
        let metrics = v.get("metrics").and_then(|m| m.as_array()).cloned().unwrap_or_default();
        let metric = metrics.iter().find(|m| m.get("name").and_then(|n| n.as_str()) == Some("usageTotalPackageCredit"));
        let Some(metric) = metric else {
            return Ok(Quota { remaining: None, parts: json!([]), sampled_at: now_iso() });
        };
        let num = |x: &Value| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok()));
        let total = metric.get("package_credit_amount").and_then(num);
        let used = metric.get("package_credit_used").and_then(num);
        let remain = metric.get("package_credit_remain").and_then(num)
            .or_else(|| match (total, used) { (Some(t), Some(u)) => Some((t - u).max(0.0)), _ => None });
        Ok(Quota { remaining: remain, parts: json!([]), sampled_at: now_iso() })
    }

    async fn refresh(&self, c: &Cred) -> Result<Cred, PlatformError> {
        let Some(rt) = c.meta.get("refresh_token").and_then(|r| r.as_str()) else { return Err(PlatformError::Auth) };
        let jwk = c.meta.get("privateJwk").cloned().unwrap_or(Value::Null);
        let verifier = c.meta.get("verifier").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let mut pairs = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", rt),
            // 客户端逆向：client_id 恒为 "codearts"（product.json 固定值，desktop/agent 相同）
            ("client_id", "codearts"),
        ];
        // 官方实现要求带 code_verifier（PKCE 续期），否则服务端拒绝/轮换失效
        if !verifier.is_empty() {
            pairs.push(("code_verifier", &verifier));
        }
        let v = sts_token(&pairs, &jwk).await?;
        // 错误响应（refresh_token 轮换作废等）
        if let Some(ec) = v.get("error_code").and_then(|e| e.as_str()) {
            let msg = v.get("error_msg").and_then(|m| m.as_str()).unwrap_or("STS refresh failed");
            if ec.contains("1806") || msg.contains("refresh token") {
                // RT 一次性且已作废（被客户端/其它实例轮换消费）——唯一出路是重新授权
                return Err(PlatformError::Identity(format!(
                    "码道授权已失效（refresh token 已被使用 {ec}）：请在「账号管理」删除该账号后重新「添加账号」授权，或先在码道客户端登录后点「扫描本机」"
                )));
            }
            return Err(PlatformError::Format(format!("STS {ec}: {msg}")));
        }
        let mut out = c.clone();
        if let Some(cred) = v.get("credentials") {
            out.meta["access_key_id"] = cred.get("access_key_id").cloned().unwrap_or(Value::Null);
            out.meta["secret_access_key"] = cred.get("secret_access_key").cloned().unwrap_or(Value::Null);
            out.meta["security_token"] = cred.get("security_token").cloned().unwrap_or(Value::Null);
        }
        // refresh_token 一次性轮换：新 RT 必须保存，否则旧 RT 立即作废
        if let Some(nrt) = v.get("refresh_token").and_then(|r| r.as_str()) {
            out.meta["refresh_token"] = json!(nrt);
        }
        // 响应结构：{ credentials: { …, expiration(ISO) }, refresh_token }
        if let Some(exp) = v.pointer("/credentials/expiration").and_then(|e| e.as_str()) {
            out.expires_at = Some(exp.to_string());
        }
        Ok(out)
    }
}
