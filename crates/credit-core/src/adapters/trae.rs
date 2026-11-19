//! Trae 适配器（api.trae.cn）。
//! headers: Authorization: Cloud-IDE-JWT {token} / x-device-id(16位数字) / x-user-region: CN
//!          / x-lscbd-platform: windows；签到 body 注入 req_source。
//! 端点：/trae/api/v2/ug/checkin_credits/{status,claim}、/trae/api/v2/pay/ide_user_ent_usage。
//! 错误码：9074 RESTRICTED、9095 设备已签。

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use p256::SecretKey;
use rand::RngCore;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::adapters::*;
use crate::error::PlatformError;
use crate::net::http::{business, platform_fetch};
use reqwest::Method;

pub const CLIENT_ID_CN: &str = "ono9krqynydwx5";
pub const CONSOLE_BASE_CN: &str = "https://www.trae.cn";
pub const APP_VERSION: &str = "3.4.9";
pub const PLUGIN_VERSION: &str = "2.3.97430";
pub const AUTH_FROM: &str = "trae";
pub const APP_TYPE: &str = "stable";

pub struct TraeAdapter {
    pub req_source: i64,
}

fn headers(c: &Cred) -> Vec<(String, String)> {
    let dev = c
        .meta
        .get("deviceId")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string();
    vec![
        ("Authorization".into(), format!("Cloud-IDE-JWT {}", c.token)),
        ("x-device-id".into(), dev),
        ("x-user-region".into(), "CN".into()),
        ("x-lscbd-platform".into(), "windows".into()),
        ("User-Agent".into(), "Trae".into()),
    ]
}

fn host_of(c: &Cred) -> String {
    c.meta
        .get("host")
        .and_then(|h| h.as_str())
        .unwrap_or("https://api.trae.cn")
        .trim_end_matches('/')
        .to_string()
}

async fn request(c: &Cred, path: &str, method: Method, body: Option<Value>) -> Result<Value, PlatformError> {
    let url = format!("{}{}", host_of(c), path);
    let hs_full = headers(c);
    let hs: Vec<(&str, String)> = hs_full.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    let mut v = platform_fetch(&url, method, body, &hs).await?;
    // Trae 网关可能包 ResponseMetadata/Result
    if v.get("Result").is_some() {
        v = v["Result"].clone();
    }
    Ok(v)
}

#[async_trait::async_trait]
impl Adapter for TraeAdapter {
    fn provider(&self) -> &'static str { "trae" }

    async fn verify(&self, c: &Cred) -> Result<Value, PlatformError> {
        let d = business(request(c, "/trae/api/v2/ug/user/info", Method::GET, None).await?)?;
        Ok(d)
    }

    async fn state(&self, c: &Cred) -> Result<CheckinState, PlatformError> {
        let d = business(request(c, "/trae/api/v2/ug/checkin_credits/status", Method::POST, Some(json!({ "req_source": self.req_source }))).await?)?;
        let flag = d.get("checked_in").or_else(|| d.get("today_checked_in"));
        let checked = match flag {
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(n)) => n.as_i64() != Some(0),
            _ => return Err(PlatformError::Format("cannot parse trae checkin state".into())),
        };
        Ok(CheckinState {
            checked,
            claimable: !checked,
            reward_amount: d.get("credits").and_then(|v| v.as_f64()).or_else(|| d.get("extra_credits").and_then(|v| v.as_f64())),
            raw: d,
            ..Default::default()
        })
    }

    async fn claim(&self, c: &Cred, st: &CheckinState) -> Result<Receipt, PlatformError> {
        if st.checked {
            return Ok(Receipt { status: "already".into(), amount: None, message: Some("本周期此前已领取".into()) });
        }
        if !st.claimable {
            return Ok(Receipt { status: "no-activity".into(), amount: None, message: Some("当前暂无可领取活动".into()) });
        }
        let d = business(
            request(c, "/trae/api/v2/ug/checkin_credits/claim", Method::POST, Some(json!({ "req_source": self.req_source }))).await?,
        )
        .map_err(|e| match &e {
            PlatformError::Business { code, .. } => match code.as_i64() {
                Some(9074) => PlatformError::Restricted("9074".into()),
                Some(9095) => PlatformError::Format("9095 device already checked".into()),
                _ => e,
            },
            _ => e,
        })?;
        Ok(Receipt {
            status: "success".into(),
            amount: d.get("credits").and_then(|v| v.as_f64()).or_else(|| d.get("extra_credits").and_then(|v| v.as_f64())),
            message: None,
        })
    }

    async fn quota(&self, c: &Cred) -> Result<Quota, PlatformError> {
        let d = business(request(c, "/trae/api/v2/pay/ide_user_ent_usage", Method::POST, Some(json!({ "require_usage": true, "req_source": 2 }))).await?)?;
        let mut parts: Vec<Value> = Vec::new();
        let mut remaining: Option<f64> = None;
        if let Some(list) = d.get("user_entitlement_pack_list").and_then(|l| l.as_array()) {
            let numeric = |v: &Value| v.as_f64();
            for p in list {
                let b = p.get("entitlement_base_info").cloned().unwrap_or(Value::Null);
                let total = b.get("quota").and_then(|q| q.get("credits_limit")).and_then(numeric);
                let used = p.get("usage").and_then(|u| u.get("credits_amount")).and_then(numeric);
                let name = p.get("group_name").and_then(|v| v.as_str())
                    .or_else(|| p.get("display_desc").and_then(|v| v.as_str()))
                    .unwrap_or("积分包");
                let expire = p.get("expire_time").or_else(|| b.get("end_time")).cloned().unwrap_or(Value::Null);
                let (rem, total_u, used_u) = match (total, used) {
                    (Some(t), Some(u)) => (Some((t - u).max(0.0)), Some(t), Some(u)),
                    (Some(t), None) => (None, Some(t), None),
                    _ => (None, None, None),
                };
                parts.push(json!({
                    "id": p.get("id").or_else(|| p.get("entitlement_pack_id")).or_else(|| b.get("id")).cloned().unwrap_or(Value::Null),
                    "name": name, "total": total_u, "used": used_u, "remaining": rem,
                    "expiresAt": expire,
                }));
            }
            if let Some(s) = d.get("usage_summary") {
                if let (Some(t), Some(u)) = (s.get("total_amount").and_then(numeric), s.get("consumed_amount").and_then(numeric)) {
                    remaining = Some((t - u).max(0.0));
                }
            }
        }
        parts.retain(|p| !p["expiresAt"].is_string() || p["expiresAt"].as_str().map(|s| !s.is_empty()).unwrap_or(true));
        let sum: f64 = parts.iter().filter_map(|p| p["remaining"].as_f64()).sum();
        Ok(Quota {
            remaining: remaining.or(if parts.is_empty() { None } else { Some(sum) }),
            parts: Value::Array(parts),
            sampled_at: now_iso(),
        })
    }

    async fn refresh(&self, c: &Cred) -> Result<Cred, PlatformError> {
        // Trae refresh：/trae/api/v3/oauth/RefreshToken（未登录流程不主动调用；占位对齐）
        let url = format!("{}/trae/api/v3/oauth/RefreshToken", host_of(c));
        let hs_full = headers(c);
    let hs: Vec<(&str, String)> = hs_full.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let v = platform_fetch(&url, Method::POST, Some(json!({ "refresh_token": c.refresh_token })), &hs).await?;
        let d = business(v)?;
        let mut out = c.clone();
        if let Some(t) = d.get("Token").or_else(|| d.get("accessToken")).and_then(|t| t.as_str()) {
            out.token = t.to_string();
        }
        Ok(out)
    }
}

// ===== 登录流程（对齐 vendor/traeAuth.mjs） =====

#[derive(Clone)]
pub struct TraeIdentity {
    pub device_id: String,
    pub machine_id: String,
    pub public_key_pem: String,
    pub private_jwk: Value,
}

/// 16 位纯数字设备号（首位 1-9）+ UUID machineId + EC P-256 identity
pub fn new_device_identity() -> TraeIdentity {
    let mut rng = rand::rngs::OsRng;
    let mut first = [0u8; 1];
    loop {
        rng.fill_bytes(&mut first);
        if (1..=9).contains(&first[0]) { break; }
    }
    let mut rest = [0u8; 15];
    rng.fill_bytes(&mut rest);
    let dev: String = std::iter::once(first[0])
        .chain(rest.iter().map(|b| b % 10))
        .map(|b| char::from(b'0' + b % 10))
        .collect();
    let sk = SecretKey::random(&mut rng);
    let pk_jwk = sk.public_key().to_jwk();
    let pem = {
        use p256::pkcs8::EncodePublicKey;
        sk.public_key().to_public_key_pem(p256::pkcs8::LineEnding::LF)
    };
    let pem = pem.unwrap_or_default();
    TraeIdentity {
        device_id: dev,
        machine_id: uuid::Uuid::new_v4().to_string(),
        public_key_pem: pem,
        private_jwk: serde_json::to_value(&pk_jwk).unwrap_or(Value::Null),
    }
}

#[derive(Clone)]
pub struct TraeLogin {
    pub trace_id: String,
    pub verifier: String,
    pub port: u16,
    pub identity: TraeIdentity,
    pub url: String,
}

pub fn start_login() -> TraeLogin {
    // 每次登录生成全新独立设备身份：多账号场景下各账号得到各自的 device_id，
    // ExchangeToken 时注册到服务端，后续签到按账号各自的设备号执行（规避
    // 「一设备一天一签」限制）。若本机 Trae 客户端有已注册设备号则优先探测。
    let identity = {
        let local = crate::adapters::trae_local::collect_local_device_ids();
        match local.first() {
            Some(id) => {
                let mut base = new_device_identity();
                base.device_id = id.clone();
                base
            }
            None => new_device_identity(),
        }
    };
    let mut vb = [0u8; 48];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut vb);
    let verifier = B64URL.encode(vb);
    let challenge = B64URL.encode(Sha256::digest(verifier.as_bytes()));
    let trace_id = uuid::Uuid::new_v4().to_string();
    let port = 3379u16; // 17388 备选；绑定失败由 server 层换
    let callback = format!("http://127.0.0.1:{port}/authorize");
    let enc = |s: &str| urlencoding_lite(s);
    let inner = format!(
        "{CONSOLE_BASE_CN}/authorization?login_version=1&auth_from={AUTH_FROM}&login_channel=native_ide&plugin_version={PLUGIN_VERSION}&auth_type=local&client_id={CLIENT_ID_CN}&redirect=0&login_trace_id={trace_id}&auth_callback_url={cb}&machine_id={mid}&device_id={did}&x_device_id={did}&x_machine_id={mid}&x_device_brand={brand}&x_device_type=win32&x_os_version={osv}&x_env=&x_app_version={APP_VERSION}&x_app_type={APP_TYPE}&code_challenge={ch}&code_challenge_method=S256&channel_name=common",
        cb = enc(&callback), mid = identity.machine_id, did = identity.device_id,
        brand = enc(&device_model()), osv = enc(&os_version()), ch = challenge,
    );
    let url = format!("{CONSOLE_BASE_CN}/login?login_platform=trae&redirect_url={}", enc(&inner));
    TraeLogin { trace_id, verifier, port, identity, url }
}



/// 按 (identity, verifier, trace_id, port) 重拼授权 URL（端口变化时用）。
pub fn rebuild_login_url(l: &TraeLogin) -> String {
    let callback = format!("http://127.0.0.1:{}/authorize", l.port);
    let enc = |s: &str| urlencoding_lite(s);
    let inner = format!(
        "{CONSOLE_BASE_CN}/authorization?login_version=1&auth_from={AUTH_FROM}&login_channel=native_ide&plugin_version={PLUGIN_VERSION}&auth_type=local&client_id={CLIENT_ID_CN}&redirect=0&login_trace_id={tid}&auth_callback_url={cb}&machine_id={mid}&device_id={did}&x_device_id={did}&x_machine_id={mid}&x_device_brand={brand}&x_device_type=win32&x_os_version={osv}&x_env=&x_app_version={APP_VERSION}&x_app_type={APP_TYPE}&code_challenge={ch}&code_challenge_method=S256&channel_name=common",
        tid = l.trace_id, cb = enc(&callback), mid = l.identity.machine_id, did = l.identity.device_id,
        brand = enc(&device_model()), osv = enc(&os_version()),
        ch = B64URL.encode(Sha256::digest(l.verifier.as_bytes())),
    );
    format!("{CONSOLE_BASE_CN}/login?login_platform=trae&redirect_url={}", enc(&inner))
}

/// 回调查询解析：剥 ? 后按 & 切 → 整体反复解码（≤3 次）→ 第一个 = 切键值
pub fn parse_callback_query(raw: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let qs = raw.split('?').last().unwrap_or(raw);
    for pair in qs.split('&') {
        if pair.is_empty() { continue; }
        let mut decoded = pair.to_string();
        for _ in 0..3 {
            let next = decode_percent(&decoded);
            if next == decoded || !next.contains('%') { decoded = next; break; }
            decoded = next;
        }
        if let Some(i) = decoded.find('=') {
            out.insert(decoded[..i].to_string(), decoded[i + 1..].to_string());
        }
    }
    out
}

pub const EXCHANGE_HOSTS: [&str; 4] = ["api.trae.cn", "api.trae.com.cn", "api.marscode.cn", "www.trae.cn"];

pub fn resolve_exchange_host(host: &str) -> Option<&'static str> {
    let h = host.trim_start_matches("https://").trim_start_matches("http://");
    let h = h.split('/').next().unwrap_or(h).split(':').next().unwrap_or(h);
    EXCHANGE_HOSTS.iter().find(|x| **x == h).copied()
}

/// ExchangeToken 三连（ExchangeToken/DeviceInfo → AuthCode → Code），首用代码可能被
/// 现代端点消耗，因此仅 404/405 允许降级。
pub async fn exchange_auth_code(login: &TraeLogin, callback: &std::collections::HashMap<String, String>) -> Result<Cred, PlatformError> {
    let host = callback.get("host").map(|h| resolve_exchange_host(h)).unwrap_or(Some("api.trae.cn"))
        .ok_or_else(|| PlatformError::Network("exchange host not allowed".into()))?;
    let auth_code = callback.get("authCode").map(|v| v.as_str()).ok_or_else(|| PlatformError::Format("missing authCode".into()))?;
    let base = format!("https://{host}");
    let attempts: Vec<(&str, String, Value)> = vec![
        ("DeviceInfo", format!("{base}/trae/api/v3/oauth/ExchangeToken"), json!({
            "ClientID": CLIENT_ID_CN, "AuthCode": auth_code, "CodeVerifier": login.verifier,
            "DeviceInfo": {
                "DeviceID": login.identity.device_id, "MachineID": login.identity.machine_id,
                "PlatformCode": "IDE_PC", "DeviceType": "PC",
                "DeviceName": username(), "DeviceModel": device_model(),
                "ClientVersion": APP_VERSION, "DevicePublicKey": login.identity.public_key_pem,
            }, "IDEVersion": APP_VERSION,
        })),
        ("AuthCode", format!("{base}/cloudide/api/v3/trae/oauth/ExchangeToken"), json!({
            "ClientID": CLIENT_ID_CN, "AuthCode": auth_code, "CodeVerifier": login.verifier,
            "DeviceID": login.identity.device_id, "PlatformCode": "IDE_PC",
        })),
        ("Code", format!("{base}/cloudide/api/v3/trae/oauth/ExchangeToken"), json!({
            "ClientID": CLIENT_ID_CN, "Code": auth_code, "CodeVerifier": login.verifier,
            "DeviceID": login.identity.device_id, "PlatformCode": "IDE_PC",
        })),
    ];
    let mut last_err = PlatformError::Network("no attempt".into());
    for (i, (tag, url, payload)) in attempts.into_iter().enumerate() {
        let hs: Vec<(&str, String)> = vec![("Content-Type", "application/json".to_string())];
        let v = match platform_fetch(&url, Method::POST, Some(payload), &hs).await {
            Ok(v) => v,
            Err(e) => return Err(PlatformError::Network(format!("TRAE 登录连接未完成 ({tag}): {e}"))),
        };
        let upstream = v.get("ResponseMetadata").and_then(|m| m.get("Error")).and_then(|e| e.get("Code")).cloned();
        let data = v.get("Result").or_else(|| v.get("data")).cloned().unwrap_or(v.clone());
        let token = pick_str(&data, &["Token", "accessToken", "access_token", "token"]);
        if token.is_some() && upstream.is_none() {
            let mut c = Cred {
                provider: "trae".into(),
                uid: callback.get("uid").cloned().unwrap_or_default(),
                name: callback.get("name").cloned().unwrap_or_else(|| "Trae 账号".into()),
                token: token.unwrap_or_default(),
                refresh_token: pick_str(&data, &["RefreshToken", "refreshToken", "refresh_token"]),
                expires_at: None,
                meta: json!({ "deviceId": login.identity.device_id, "machineId": login.identity.machine_id, "host": base, "reqSource": 1 }),
            };
            if c.uid.is_empty() {
                c.uid = jwt_uid(&c.token);
            }
            return Ok(c);
        }
        let status_forbidden_fallback = i == 0; // 仅首个端点响应非 404/405 时禁止降级——此处简化为：非网络错误即停
        last_err = PlatformError::Format(format!("exchange {tag} failed: upstream={upstream:?} token={token:?} stop={status_forbidden_fallback}"));
        if upstream.is_some() { break; }
    }
    Err(last_err)
}

fn pick_str(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| v.get(*k).and_then(|x| x.as_str()).map(|s| s.to_string()))
}

fn jwt_uid(token: &str) -> String {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 { return String::new(); }
    let Ok(bytes) = B64URL.decode(parts[1]) else { return String::new() };
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    v.get("data").and_then(|d| d.get("id")).and_then(|i| i.as_str()).unwrap_or("").to_string()
}

pub fn urlencoding_lite(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn decode_percent(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = (bytes[i + 1] as char).to_digit(16);
            let l = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (h, l) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn device_model() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into())
}

pub fn os_version() -> String {
    std::env::var("OS").unwrap_or_else(|_| "Windows".into())
}

pub fn username() -> String {
    std::env::var("USERNAME").unwrap_or_else(|_| "user".into())
}

