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

/// 华为云 STS 端点（token 签发/续期 + caller-identity 校验）
pub const STS_BASE: &str = "https://sts.cn-north-4.myhuaweicloud.com";
/// 码道业务 API host（ops delivery/claim/confirm、用量统计）
pub const API_BASE: &str = "https://snap-access.cn-north-4.myhuaweicloud.com";

/// 从 caller-identity 响应解析身份字段（对齐官方客户端 apiService.getUserInfo）：
/// - userId = principal_id || user_id || userId
/// - userName = principal_urn 最后一个 ':' 之后的部分（再去掉 '/' 前缀），如
///   `iam::<account>:user:hw083373957922722` → `hw083373957922722`
/// - domainId = account_id || domain_id || domainId
///
/// 旧实现用 `rsplit(":user:").nth(1)`，对该 URN 会取到 `iam::<account>` 前缀，
/// 导致账号名显示为 `iam::01a1e...`（“无法获取账号信息”的直接原因）。
pub fn identity_of(v: &Value) -> (String, String, String) {
    let uid = ["principal_id", "user_id", "userId"]
        .iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_str()))
        .unwrap_or("")
        .to_string();
    let mut name = String::new();
    if let Some(urn) = v.get("principal_urn").and_then(|x| x.as_str()) {
        if let Some(seg) = urn.rsplit(':').next() {
            name = seg.to_string();
        }
    }
    if name.is_empty() {
        name = ["user_name", "userName"]
            .iter()
            .find_map(|k| v.get(*k).and_then(|x| x.as_str()))
            .unwrap_or("")
            .to_string();
    }
    if let Some(slash) = name.find('/') {
        name = name[slash + 1..].to_string();
    }
    let domain = ["account_id", "domain_id", "domainId"]
        .iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_str()))
        .unwrap_or("")
        .to_string();
    (uid, name, domain)
}

/// epoch 毫秒 → ISO 字符串（UTC+8，落库/展示统一格式）
pub fn epoch_ms_to_iso(ms: i64) -> String {
    // 与 now_iso 一致：按 UTC+8 偏移表达
    let secs = ms / 1000 + 8 * 3600;
    let days = secs / 86400;
    let rem = secs % 86400;
    let (y, mo, d) = crate::adapters::civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}.000+08:00", rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// ISO 8601（"2026-11-19T12:16:00.000Z" / 无毫秒 / 带偏移）→ epoch 毫秒。
/// 用于把 STS 返回的 expiration 同步回客户端 secretStorage 的 expiresAt（毫秒）。
pub fn iso_to_epoch_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (date, rest) = s.split_once('T')?;
    let mut dparts = date.split('-');
    let y: i64 = dparts.next()?.parse().ok()?;
    let mo: i64 = dparts.next()?.parse().ok()?;
    let day: i64 = dparts.next()?.parse().ok()?;
    // 解析时区后缀：Z = UTC；+HH:MM / -HH:MM = 本地时间相对 UTC 的偏移
    let mut time = rest;
    let mut offset_secs: i64 = 0;
    if let Some(stripped) = time.strip_suffix('Z') {
        time = stripped;
    } else if let Some(pos) = time.find('+') {
        offset_secs = parse_tz_offset(&time[pos + 1..])?;
        time = &time[..pos];
    } else if let Some(pos) = time.rfind('-') {
        // 时间部分里的 '-' 只可能是时区偏移分隔（日期已在前面拆掉）
        if pos > 2 {
            offset_secs = -parse_tz_offset(&time[pos + 1..])?;
            time = &time[..pos];
        }
    }
    let (hms, frac) = time.split_once('.').unwrap_or((time, ""));
    let mut tparts = hms.split(':');
    let h: i64 = tparts.next()?.parse().ok()?;
    let mi: i64 = tparts.next().unwrap_or("0").parse().ok()?;
    let se: i64 = tparts.next().unwrap_or("0").parse().ok()?;
    let ms: i64 = if frac.is_empty() {
        0
    } else {
        let digits: String = frac.chars().take(3).collect();
        let padded = format!("{digits:0<3}");
        padded.parse().unwrap_or(0)
    };
    let days = crate::adapters::days_from_civil(y, mo, day);
    let local = days * 86400 + h * 3600 + mi * 60 + se;
    Some(((local - offset_secs) * 1000) + ms)
}

/// 解析 "08:00" / "0800" / "8" 形式的时区偏移为秒
fn parse_tz_offset(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some((h, m)) = s.split_once(':') {
        let h: i64 = h.parse().ok()?;
        let m: i64 = m.parse().ok()?;
        return Some(h * 3600 + m * 60);
    }
    if s.len() == 4 && s.chars().all(|c| c.is_ascii_digit()) {
        let h: i64 = s[..2].parse().ok()?;
        let m: i64 = s[2..].parse().ok()?;
        return Some(h * 3600 + m * 60);
    }
    Some(s.parse::<i64>().ok()? * 3600)
}

/// 码道适配器；desktop=true 走 DESKTOP 渠道（否则 IDE）
pub struct CodeArtsAdapter { pub desktop: bool }

// ===== DPoP =====

/// DPoP 私钥 JWK 归一化：本机 secretStorage 里 `loginContext.dpopKeyPair.privateKey`
/// 是 **JSON 字符串**（"{\"kty\":\"EC\",...}"），而登录流程写入的是对象。
/// 旧实现直接取 json 字段 d，遇到字符串就报 `jwk missing d` —— 这是「码道账号无法
/// 自动续期」的根因（DPoP 签名永远失败）。这里对两种形态做兼容（最多解两层字符串）。
pub fn jwk_value(v: &Value) -> Value {
    let mut cur = v.clone();
    for _ in 0..2 {
        if let Value::String(s) = &cur {
            if let Ok(parsed) = serde_json::from_str::<Value>(s) {
                cur = parsed;
                continue;
            }
        }
        break;
    }
    cur
}

/// 构造 DPoP proof JWT：ES256（p1363 r||s 签名），payload 带 htu/htm/iat/jti
pub fn dpop_header(private_jwk: &Value, htu: &str) -> Result<String, PlatformError> {
    let jwk = jwk_value(private_jwk);
    let sk = SigningKey::from(JwkDec::from_jwk(&jwk)?);
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

/// 由签名私钥导出公钥 JWK（内嵌进 DPoP header）
pub fn public_jwk_of(sk: &SigningKey) -> Value {
    let vk = sk.verifying_key();
    let enc = vk.to_encoded_point(false); // uncompressed: 04||X||Y
    let bytes = enc.as_bytes();
    let x = b64url_encode(&bytes[1..33]);
    let y = b64url_encode(&bytes[33..65]);
    json!({ "kty": "EC", "crv": "P-256", "x": x, "y": y })
}

/// base64url 解码（失败转 PlatformError::Format）
pub fn b64url_decode(s: &str) -> Result<Vec<u8>, PlatformError> {
    B64URL.decode(s).map_err(|e| PlatformError::Format(format!("b64url: {e}")))
}
/// base64url 编码
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
    let _hs = [("Content-Type", "application/x-www-form-urlencoded".to_string()),
        ("DPoP", dpop.clone())];
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(std::time::Duration::from_secs(15)).build().unwrap();
    let resp = client.post(&url).header("Content-Type", "application/x-www-form-urlencoded").header("DPoP", dpop).body(body)
        .send().await.map_err(|e| PlatformError::Network(e.to_string()))?;
    match resp.status().as_u16() { 401 => return Err(PlatformError::Auth), 429 => return Err(PlatformError::Rate), _ => {} }
    let v: Value = resp.json().await.map_err(|e| PlatformError::Format(e.to_string()))?;
    Ok(v)
}
// ===== SDK-HMAC-SHA256 请求签名 =====

/// percent 解码（供 path 规范化 encode(decode(seg)) 使用）
pub fn decode_percent(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 1 && i + 2 <= b.len() - 1 + 1 && i + 2 < b.len() {
            let h = (b[i + 1] as char).to_digit(16).unwrap_or(99);
            let l = (b[i + 2] as char).to_digit(16).unwrap_or(99);
            if h < 16 && l < 16 { out.push(((h << 4) | l) as u8); i += 3; continue; }
        }
        out.push(b[i]); i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// 参数与 HWS 签名要素一一对应，不拆结构体
#[allow(clippy::too_many_arguments)]
/// SDK-HMAC-SHA256 请求签名：返回 Authorization 头与需随请求携带的 header 集
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
    sign_headers.sort_by_key(|a| a.0.to_lowercase());
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

/// 小写 hex 编码
pub fn hex_lower(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ===== 业务请求 =====

fn channel(desktop: bool) -> &'static str { if desktop { "DESKTOP" } else { "IDE" } }

/// 统一业务请求：HWS 签名发请求；HTTP 200 里的网关 error_code 按语义映射（06xx/1806/过期→Auth）
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

    /// 校验 AK/SK/STS 临时凭据：caller-identity 查询当前凭据归属（走 STS 域签名）
    async fn verify(&self, c: &Cred) -> Result<Value, PlatformError> {
        // GET /v5/caller-identity（on STS）
        let v = ops_request(c, self.desktop, Method::GET, "/v5/caller-identity", "", Value::Null, true).await?;
        Ok(v)
    }

    /// 查签到活动：/v1/ops/delivery 过滤 campaignId 非空且 claimable!==false 的项
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

    /// 逐 campaign 领取：幂等键 claim_{id}_{epoch_ms}，成功后按回执补 confirm（失败不阻塞）
    async fn claim(&self, c: &Cred, st: &CheckinState) -> Result<Receipt, PlatformError> {
        if st.checked {
            return Ok(Receipt { status: "already".into(), amount: None, message: None });
        }
        let items = st.raw.get("items").and_then(|x| x.as_array()).cloned().unwrap_or_default();
        if items.is_empty() {
            return Ok(Receipt { status: "no-activity".into(), amount: None, message: Some("当前暂无可领取的签到活动（该平台此刻没有可领奖励，或今日已领完）".into()) });
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

    /// 查用量：/snap-manager/v1/statistics/plugin 取 usageTotalPackageCredit 指标
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

    /// STS refresh_token 续期（须带 PKCE verifier + DPoP）；RT 一次性轮换，作废需重新授权。
    ///
    /// 修复点：
    /// - RT 优先取凭据顶层 `refresh_token`（本机扫描/同步导入只写顶层，旧实现只读 meta，
    ///   导致「本机导入的账号永远无法自动续期」）；
    /// - client_id 按通道区分：desktop = `codearts`，IDE/agent = `codearts-agent`
    ///   （与 login.rs / 客户端 product.json 一致）；
    /// - 新 RT 同时写回顶层与 meta，避免两种写入路径互相覆盖后丢 RT。
    async fn refresh(&self, c: &Cred) -> Result<Cred, PlatformError> {
        let rt = c
            .refresh_token
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| c.meta.get("refresh_token").and_then(|r| r.as_str()))
            .ok_or(PlatformError::Auth)?
            .to_string();
        let jwk = jwk_value(&c.meta.get("privateJwk").cloned().unwrap_or(Value::Null));
        if !jwk.is_object() || jwk.get("d").and_then(|d| d.as_str()).unwrap_or("").is_empty() {
            return Err(PlatformError::Identity(
                "码道凭据缺少 DPoP 私钥（privateJwk），无法续期：请重新授权或在码道客户端登录后「扫描本机」".into(),
            ));
        }
        let verifier = c.meta.get("verifier").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let client_id = if self.desktop { "codearts" } else { "codearts-agent" };
        let mut pairs = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", rt.as_str()),
            ("client_id", client_id),
        ];
        // 官方实现要求带 code_verifier（PKCE 续期），否则服务端拒绝/轮换失效
        if !verifier.is_empty() {
            pairs.push(("code_verifier", verifier.as_str()));
        }
        let v = sts_token(&pairs, &jwk).await?;
        // 错误响应：OAuth 风格 {error:{message}} 或 HWS 风格 {error_code,error_msg}
        if let Some(err) = v.get("error") {
            let msg = err
                .get("message")
                .or_else(|| v.get("error_description"))
                .and_then(|m| m.as_str())
                .unwrap_or("STS refresh failed")
                .to_string();
            let code = err.get("code").or_else(|| v.get("error_code")).and_then(|c| c.as_str()).unwrap_or("");
            if code.contains("1806") || msg.contains("refresh token") || msg.contains("invalid_grant") {
                return Err(PlatformError::Identity(
                    "码道授权已失效（refresh token 已被轮换/作废）：请重新在码道客户端登录后点「扫描本机」，或在「账号管理」重新添加账号授权".into(),
                ));
            }
            return Err(PlatformError::Format(format!("STS refresh {code}: {msg}")));
        }
        if let Some(ec) = v.get("error_code").and_then(|e| e.as_str()) {
            let msg = v.get("error_msg").and_then(|m| m.as_str()).unwrap_or("STS refresh failed");
            if ec.contains("1806") || msg.contains("refresh token") {
                return Err(PlatformError::Identity(format!(
                    "码道授权已失效（refresh token 已被使用 {ec}）：请在码道客户端重新登录后点「扫描本机」，或重新添加账号授权"
                )));
            }
            return Err(PlatformError::Format(format!("STS {ec}: {msg}")));
        }
        let cred = v.get("credentials").or_else(|| v.get("result").and_then(|r| r.get("credentials")));
        let Some(cred) = cred else {
            return Err(PlatformError::Format(format!(
                "STS refresh 响应缺少 credentials：{}",
                v.to_string().chars().take(200).collect::<String>()
            )));
        };
        let mut out = c.clone();
        out.meta["access_key_id"] = cred.get("access_key_id").cloned().unwrap_or(Value::Null);
        out.meta["secret_access_key"] = cred.get("secret_access_key").cloned().unwrap_or(Value::Null);
        out.meta["security_token"] = cred.get("security_token").cloned().unwrap_or(Value::Null);
        // refresh_token 一次性轮换：新 RT 必须保存（顶层 + meta 同步），否则旧 RT 立即作废
        if let Some(nrt) = v.get("refresh_token").and_then(|r| r.as_str()) {
            if !nrt.is_empty() {
                out.refresh_token = Some(nrt.to_string());
                out.meta["refresh_token"] = json!(nrt);
            }
        }
        // 响应结构：{ credentials: { …, expiration(ISO) }, refresh_token }
        if let Some(exp) = cred.get("expiration").and_then(|e| e.as_str()) {
            out.expires_at = Some(exp.to_string());
        }
        Ok(out)
    }
}
