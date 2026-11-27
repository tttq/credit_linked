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

/// 登录/换码共用的官方 OAuth client_id（CN 区桌面客户端固定值，product.json 缺省时兜底）
pub const CLIENT_ID_CN: &str = "ono9krqynydwx5";
/// 控制台站点域名（登录/授权页所在，product.json 缺省时兜底）
pub const CONSOLE_BASE_CN: &str = "https://www.trae.cn";
/// 登录参数上报的桌面客户端版本号（product.json `appVersion` 缺省时兜底）
pub const APP_VERSION: &str = "3.4.22";
/// 登录参数上报的 IDE 插件版本号（product.json `tronBuildVersion` 缺省时兜底）
pub const PLUGIN_VERSION: &str = "2.3.105403";
/// 换码/取用户信息固定走 API 域（www.trae.cn 是前端页面，POST 会回 HTML）
pub const API_BASE_CN: &str = "https://api.trae.cn";
/// ExchangeToken 端点（登录换码与 refresh 续期共用）
pub const EXCHANGE_PATH: &str = "/trae/api/v3/oauth/ExchangeToken";
/// 取用户信息端点（uid/昵称的真值来源）
pub const USERINFO_PATH: &str = "/cloudide/api/v3/trae/GetUserInfo";
/// 授权页 auth_from 渠道标识
pub const AUTH_FROM: &str = "trae";
/// 授权页 x_app_type（发行通道）
pub const APP_TYPE: &str = "stable";

/// Trae 签到/额度适配器；req_source 区分桌面客户端(1)与 SOLO(2)，签到 body 需注入
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

    /// 校验凭据：GET /user/info（business 错误透传给上层）
    async fn verify(&self, c: &Cred) -> Result<Value, PlatformError> {
        let d = business(request(c, "/trae/api/v2/ug/user/info", Method::GET, None).await?)?;
        Ok(d)
    }

    /// 查签到状态：checked_in 字段可能是 bool 或数字（0/非 0）
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

    /// 领取签到积分；业务码 9074=设备受限(Restricted)、9095=设备今日已签
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
                // 1001/1002 = TRAE 官方鉴权错误段（1001 鉴权失败 / 1002 令牌过期）
                // → 映射为 Auth，让上层统一走 refresh_token 续期链路后重试
                Some(1001) | Some(1002) => PlatformError::Auth,
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

    /// 查积分包：逐 entitlement pack 拆 parts，usage_summary 兜底整体 remaining
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
        // 剔除 expiresAt 为空字符串的包（无到期信息）
        parts.retain(|p| !p["expiresAt"].is_string() || p["expiresAt"].as_str().map(|s| !s.is_empty()).unwrap_or(true));
        let sum: f64 = parts.iter().filter_map(|p| p["remaining"].as_f64()).sum();
        Ok(Quota {
            remaining: remaining.or(if parts.is_empty() { None } else { Some(sum) }),
            parts: Value::Array(parts),
            sampled_at: now_iso(),
        })
    }

    /// 续期：有设备私钥走 v3 DeviceProof 签名端点，否则回退旧契约端点
    async fn refresh(&self, c: &Cred) -> Result<Cred, PlatformError> {
        // Trae 官方续期（对齐 `marscode/request.js::exchangeTokenByRefreshToken`）：
        //   POST {api}/trae/api/v3/oauth/ExchangeToken
        //   body: ClientID + ClientSecret:"" + RefreshToken + DeviceInfo + DeviceProof
        //   DeviceProof = sign_sha256("POST\n{path}\n{clientId}\n{refreshToken}\n{timestamp}\n{nonce}")
        // 本机扫描导入的账号带 storage 里的设备密钥对；**应用内 OAuth 登录的账号**
        // 没有本机客户端密钥，必须用登录时持久化在 `meta` 里的私钥，否则续期必失败。
        let refresh_token = c.refresh_token.clone().unwrap_or_default();
        if refresh_token.is_empty() {
            return Err(PlatformError::Identity("Trae 凭据缺少 refresh_token，请重新登录 Trae".into()));
        }
        let host = host_of(c);
        let base = host.trim_end_matches('/').to_string();
        let meta_app_version = c.meta.get("appVersion").and_then(|v| v.as_str()).map(|s| s.to_string());

        // ① 优先用凭据 meta 里的登录期设备身份（OAuth 登录的账号只有这条路能续）
        let meta_identity = {
            let did = c.meta.get("deviceId").and_then(|v| v.as_str()).unwrap_or("");
            let mid = c.meta.get("machineId").and_then(|v| v.as_str()).unwrap_or("");
            let pk = c.meta.get("privateKeyPem").and_then(|v| v.as_str()).unwrap_or("");
            let pubk = c.meta.get("publicKeyPem").and_then(|v| v.as_str()).unwrap_or("");
            if did.is_empty() || mid.is_empty() || pk.is_empty() {
                None
            } else {
                Some((did.to_string(), mid.to_string(), pk.to_string(), pubk.to_string()))
            }
        };
        // ② 回落本机客户端安装目录里的设备身份
        let data_dir = crate::adapters::trae_local::trae_data_dir();
        let local_identity = data_dir.as_deref().and_then(crate::adapters::trae_local::read_identity);
        let local_keypair = data_dir.as_deref().and_then(crate::adapters::trae_local::read_device_keypair);

        let (device_id, machine_id, priv_pem, pub_pem, app_version) = if let Some((did, mid, pk, pubk)) = meta_identity {
            (did, mid, pk, pubk, meta_app_version.clone().unwrap_or_else(|| APP_VERSION.to_string()))
        } else if let (Some(id), Some(kp)) = (local_identity.as_ref(), local_keypair.as_ref()) {
            (id.device_id.clone(), id.machine_id.clone(), kp.private_key_pem.clone(), kp.public_key_pem.clone(),
             id.app_version.clone().unwrap_or_else(|| APP_VERSION.to_string()))
        } else {
            (String::new(), String::new(), String::new(), String::new(), APP_VERSION.to_string())
        };

        if !priv_pem.is_empty() {
            let path = EXCHANGE_PATH;
            let client_id = crate::adapters::trae_local::product_meta().client_id
                .unwrap_or_else(|| CLIENT_ID_CN.to_string());
            if let Some((sig, ts, nonce)) =
                crate::adapters::trae_local::sign_device_proof(&priv_pem, path, &client_id, &refresh_token)
            {
                let si = system_information();
                let mut dev = device_info_from(&device_id, &machine_id, &pub_pem, &app_version, &si);
                if let Some(o) = dev.as_object_mut() { o.remove("DeviceName"); }
                let body = json!({
                    "ClientID": client_id,
                    "ClientSecret": "",
                    "RefreshToken": refresh_token,
                    "DeviceInfo": dev,
                    "DeviceProof": { "Signature": sig, "Timestamp": ts, "Nonce": nonce },
                    "IDEVersion": app_version,
                });
                let url = format!("{base}{path}");
                let hs: Vec<(&str, String)> = vec![
                    ("Content-Type", "application/json".to_string()),
                    ("x-cloudide-token", String::new()),
                ];
                let v = platform_fetch(&url, Method::POST, Some(body), &hs).await?;
                // 响应信封：{ResponseMetadata, Result:{Token, RefreshToken, TokenExpireAt, ...}}
                if let Some(code) = v.pointer("/ResponseMetadata/Error/Code").and_then(|c| c.as_str()) {
                    // 官方 W$ 集合：刷新令牌失效（需要重新登录）
                    const RT_INVALID: [&str; 7] = ["20324", "20101", "20315", "20125", "20126", "20401", "20403"];
                    if RT_INVALID.contains(&code) {
                        return Err(PlatformError::Identity(format!(
                            "Trae 刷新令牌已失效（{code}），请重新登录该账号"
                        )));
                    }
                }
                let d = v.get("Result").cloned().unwrap_or(v);
                if let Some(t) = pick_str(&d, &["Token", "accessToken", "access_token"]) {
                    let mut out = c.clone();
                    out.token = t.to_string();
                    if let Some(rt) = pick_str(&d, &["RefreshToken", "refreshToken"]) { out.refresh_token = Some(rt); }
                    if let Some(ms) = d.get("TokenExpireAt").and_then(|v| v.as_i64()) {
                        out.expires_at = Some(iso_from_ms_pub(ms as u64));
                    }
                    // 补全 meta（老账号从本机扫描来、可能没存私钥）
                    if let Some(o) = out.meta.as_object_mut() {
                        if o.get("deviceId").is_none() { o.insert("deviceId".into(), json!(device_id)); }
                        if o.get("machineId").is_none() { o.insert("machineId".into(), json!(machine_id)); }
                        if o.get("privateKeyPem").is_none() && !priv_pem.is_empty() {
                            o.insert("privateKeyPem".into(), json!(priv_pem));
                        }
                        if o.get("publicKeyPem").is_none() && !pub_pem.is_empty() {
                            o.insert("publicKeyPem".into(), json!(pub_pem));
                        }
                        o.insert("appVersion".into(), json!(app_version));
                    }
                    return Ok(out);
                }
            }
        }
        // 旧契约回退：POST {host}/cloudide/api/v3/trae/oauth/ExchangeToken
        let legacy = json!({
            "ClientID": "ono9krqynydwx5",
            "ClientSecret": "-",
            "RefreshToken": refresh_token,
            "UserID": c.uid,
        });
        let url = format!("{base}/cloudide/api/v3/trae/oauth/ExchangeToken");
        let hs: Vec<(&str, String)> = vec![("Content-Type", "application/json".to_string())];
        let v = platform_fetch(&url, Method::POST, Some(legacy), &hs).await?;
        let d = v.get("Result").cloned().unwrap_or(v);
        let mut out = c.clone();
        if let Some(t) = pick_str(&d, &["Token", "accessToken", "access_token"]) {
            out.token = t.to_string();
        } else {
            return Err(PlatformError::Identity("Trae 续期失败：响应中没有新 token，请重新登录 Trae".into()));
        }
        if let Some(rt) = pick_str(&d, &["RefreshToken", "refreshToken"]) { out.refresh_token = Some(rt); }
        if let Some(ms) = d.get("TokenExpireAt").and_then(|v| v.as_i64()) {
            out.expires_at = Some(iso_from_ms_pub(ms as u64));
        }
        Ok(out)
    }
}

// ===== 登录流程（对齐 vendor/traeAuth.mjs） =====

#[derive(Clone)]
/// 登录期新生成的设备身份：设备号 + machineId + EC P-256 密钥对（ExchangeToken 注册用）
pub struct TraeIdentity {
    pub device_id: String,
    pub machine_id: String,
    pub public_key_pem: String,
    pub private_jwk: Value,
    /// PKCS#8 私钥 PEM：refresh 时要用它对 DeviceProof 签名（必须持久化到凭据 meta 里，
    /// 否则重启后无法续期）
    pub private_key_pem: String,
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
    let sk_pem = {
        use p256::pkcs8::EncodePrivateKey;
        sk.to_pkcs8_pem(p256::pkcs8::LineEnding::LF)
            .map(|s| s.to_string())
            .unwrap_or_default()
    };
    TraeIdentity {
        device_id: dev,
        machine_id: uuid::Uuid::new_v4().to_string(),
        public_key_pem: pem,
        private_jwk: serde_json::to_value(&pk_jwk).unwrap_or(Value::Null),
        private_key_pem: sk_pem,
    }
}

/// 官方 `getSystemInformation()` 的等价物：登录 URL 与 DeviceInfo 都要用它
#[derive(Clone, Default)]
pub struct SysInfo {
    pub device_model: String,
    pub device_manufacturer: String,
    pub cpu_brand: String,
    pub os_name: String,
    pub os_version: String,
}

/// 本机系统信息（对齐官方 `iCubeSystemInformationService` 的 win32 分支）：
/// - `osName = Ch==="win32" ? "windows"` 
/// - `osVersion = os.version()`（Node 在 Windows 上返回产品名，如 “Windows 11 家庭中文版”）
/// - `deviceModel/deviceManufacturer` 来自 electron `ahaDeviceService.deviceInfo`（即 CIM 的
///   `Win32_ComputerSystem.Model/Manufacturer`）
/// - `cpuBrand = os.cpus()[0].model` 去掉 “@ 3.70GHz” 后缀
///
/// 注意：Windows 11 24H2 起 `wmic` 已被移除，所以必须走 PowerShell CIM，否则三个字段全空
/// —— 空 DeviceModel/DeviceBrand 是很明显的假客户端特征。
pub fn system_information() -> SysInfo {
    // CIM 查询要起一次 PowerShell（数百毫秒），设备信息在同一次运行里不会变 → 缓存
    static CACHE: std::sync::OnceLock<SysInfo> = std::sync::OnceLock::new();
    if let Some(v) = CACHE.get() {
        return v.clone();
    }
    let si = collect_system_information();
    let _ = CACHE.set(si.clone());
    si
}

fn collect_system_information() -> SysInfo {
    let mut si = SysInfo {
        os_name: if cfg!(windows) { "windows".to_string() } else { std::env::consts::OS.to_string() },
        ..Default::default()
    };
    if cfg!(windows) {
        // 一次 PowerShell 取齐：Model / Manufacturer / CPU / ProductName
        let script = "$ErrorActionPreference='SilentlyContinue';\
$cs=Get-CimInstance Win32_ComputerSystem;\
$cpu=Get-CimInstance Win32_Processor|Select-Object -First 1;\
$pn=(Get-ItemProperty 'HKLM:\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion').ProductName;\
Write-Output ($cs.Model);Write-Output ($cs.Manufacturer);Write-Output ($cpu.Name);Write-Output $pn";
        let lines = ps_lines(script);
        if lines.len() >= 4 {
            si.device_model = clean_field(&lines[0]);
            si.device_manufacturer = clean_field(&lines[1]);
            si.cpu_brand = strip_cpu_suffix(&clean_field(&lines[2]));
            si.os_version = clean_field(&lines[3]);
        }
        // PowerShell 不可用时回落 wmic（老系统）
        if si.device_model.is_empty() {
            si.device_model = clean_field(&wmic_first(&["computersystem", "get", "model"]));
        }
        if si.device_manufacturer.is_empty() {
            si.device_manufacturer = clean_field(&wmic_first(&["computersystem", "get", "manufacturer"]));
        }
        if si.cpu_brand.is_empty() {
            si.cpu_brand = strip_cpu_suffix(&clean_field(&wmic_first(&["cpu", "get", "name"])));
        }
    } else {
        si.os_version = crate::adapters::trae_local::os_version_string();
    }
    si
}

/// 跑一段 PowerShell 并返回非空输出行（隐藏控制台窗口）
fn ps_lines(script: &str) -> Vec<String> {
    let mut cmd = std::process::Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", script]);
    match crate::proc_util::hide_console(cmd).output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// wmic 取首行数据（老 Windows 兜底；新系统 wmic 不存在会返回空串）
fn wmic_first(args: &[&str]) -> String {
    let mut cmd = std::process::Command::new("wmic");
    cmd.args(args);
    match crate::proc_util::hide_console(cmd).output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .skip(1)
            .map(|l| l.trim())
            .find(|l| !l.is_empty())
            .unwrap_or("")
            .to_string(),
        Err(_) => String::new(),
    }
}

/// 去掉 CIM 空值占位（“To Be Filled By O.E.M.” / “System Product Name” 之类保留，官方也是原样发）
fn clean_field(s: &str) -> String {
    let t = s.trim().trim_matches('"').trim();
    if t.eq_ignore_ascii_case("none") || t.eq_ignore_ascii_case("null") { String::new() } else { t.to_string() }
}

/// 官方 `yZe()` 同样会剥掉 “@ 3.70GHz”：`t.indexOf("@")>=0 ? t.split("@")[0].trim() : t.trim()`
fn strip_cpu_suffix(s: &str) -> String {
    match s.find('@') {
        Some(i) => s[..i].trim().to_string(),
        None => s.trim().to_string(),
    }
}

/// 官方 `DeviceInfo` 全字段（含 DeviceBrand/DeviceCPU/OSInfo/OSVersion，缺这些会被判 10101 无效参数）
pub fn device_info(identity: &TraeIdentity, app_version: &str, si: &SysInfo) -> Value {
    device_info_from(&identity.device_id, &identity.machine_id, &identity.public_key_pem, app_version, si)
}

/// 同上，但从散字段构造（续期时设备身份来自凭据 meta，没有 TraeIdentity 结构体）
pub fn device_info_from(device_id: &str, machine_id: &str, public_key_pem: &str, app_version: &str, si: &SysInfo) -> Value {
    json!({
        "DeviceID": device_id,
        "MachineID": machine_id,
        "PlatformCode": "IDE_PC",
        "DeviceType": "PC",
        "DeviceName": crate::adapters::trae_local::user_display_name(),
        "DeviceModel": si.device_model,
        "ClientVersion": app_version,
        "DevicePublicKey": public_key_pem,
        "DeviceBrand": si.device_manufacturer,
        "DeviceCPU": si.cpu_brand,
        "OSInfo": si.os_name,
        "OSVersion": si.os_version,
    })
}

#[derive(Clone)]
/// 一次登录会话：PKCE verifier + 回调端口 + 授权 URL
pub struct TraeLogin {
    pub trace_id: String,
    pub verifier: String,
    pub port: u16,
    pub identity: TraeIdentity,
    pub url: String,
}

/// 官方 `buildLoginUrl` 的等价实现（`out-build/vs/code/electron-main/oauth/userLogin/loginUrlBuilder.js`）：
/// ```text
/// {consoleHost}/authorization?login_version=1&auth_from=trae&login_channel=native_ide
///   &plugin_version={tronBuildVersion||"local"}&auth_type=local&client_id={stable}
///   &redirect=0&login_trace_id={uuid}&auth_callback_url=http://127.0.0.1:{port}/authorize
///   &machine_id={mid}&device_id={did}
///   &x_device_id={did}&x_machine_id={mid}&x_device_brand={deviceModel}&x_device_type={osName}
///   &x_os_version={osVersion}&x_app_version={appVersion}&x_app_type=stable
///   &code_challenge={ch}&code_challenge_method=S256
///   &doubao_passport=1&loginScope=toc      ← TRAE 账号必须有，否则授权页不会回调
/// ```
/// 注意：官方**不再**套 `/login?login_platform=…&redirect_url=…` 外层，是直达 `/authorization`。
fn build_login_url(l: &TraeLogin) -> String {
    let meta = crate::adapters::trae_local::product_meta();
    let console_host = meta.console_host.clone().unwrap_or_else(|| CONSOLE_BASE_CN.to_string());
    let console_host = console_host.trim_end_matches('/').to_string();
    let client_id = meta.client_id.clone().unwrap_or_else(|| CLIENT_ID_CN.to_string());
    let plugin_version = meta.tron_version.clone().unwrap_or_else(|| "local".to_string());
    let app_version = meta.app_version.clone().unwrap_or_else(|| APP_VERSION.to_string());
    let si = system_information();
    let challenge = B64URL.encode(Sha256::digest(l.verifier.as_bytes()));
    let callback = format!("http://127.0.0.1:{}/authorize", l.port);
    format!(
        "{host}/authorization?login_version=1&auth_from={AUTH_FROM}&login_channel=native_ide\
         &plugin_version={plugin_version}&auth_type=local&client_id={client_id}&redirect=0\
         &login_trace_id={tid}&auth_callback_url={cb}&machine_id={mid}&device_id={did}\
         &x_device_id={did}&x_machine_id={mid}&x_device_brand={brand}&x_device_type={osname}\
         &x_os_version={osv}&x_app_version={app_version}&x_app_type={APP_TYPE}\
         &code_challenge={ch}&code_challenge_method=S256&doubao_passport=1&loginScope=toc",
        host = console_host,
        tid = l.trace_id,
        cb = urlencoding_lite(&callback),
        mid = l.identity.machine_id,
        did = l.identity.device_id,
        brand = urlencoding_lite(&si.device_model),
        osname = urlencoding_lite(&si.os_name),
        osv = urlencoding_lite(&si.os_version),
        ch = challenge,
    )
}

/// 发起登录：生成全新设备身份 + PKCE，并拼出 console 授权页 URL
pub fn start_login() -> TraeLogin {
    // 每次登录生成全新独立设备身份：多账号场景下各账号得到各自的 device_id，
    // ExchangeToken(DeviceInfo) 时注册到服务端并绑定到本账号。绝不能复用本机
    // Trae 客户端/其他账号已注册的设备号——同一设备号重复注册会把旧账号的
    // 设备绑定顶掉（表现为「登录第二个账号挤掉第一个」），且违反
    // 「一设备一天一签」限制。
    let identity = new_device_identity();
    let mut vb = [0u8; 48];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut vb);
    let verifier = B64URL.encode(vb);
    let trace_id = uuid::Uuid::new_v4().to_string();
    let port = 3379u16; // 17388 备选；绑定失败由 server 层换
    let mut l = TraeLogin { trace_id, verifier, port, identity, url: String::new() };
    l.url = build_login_url(&l);
    l
}

/// 按 (identity, verifier, trace_id, port) 重拼授权 URL（端口变化时用）。
pub fn rebuild_login_url(l: &TraeLogin) -> String {
    build_login_url(l)
}

/// 回调查询解析：剥 ? 后按 & 切 → 整体反复解码（≤3 次）→ 第一个 = 切键值
pub fn parse_callback_query(raw: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let qs = raw.split('?').next_back().unwrap_or(raw);
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

/// 允许换码的主机白名单（回调里的 host/consoleHost 归一化后必须命中其一）
pub const EXCHANGE_HOSTS: [&str; 4] = ["api.trae.cn", "api.trae.com.cn", "api.marscode.cn", "www.trae.cn"];

/// 归一化 host 并匹配白名单；未命中返回 None
pub fn resolve_exchange_host(host: &str) -> Option<&'static str> {
    let h = host.trim_start_matches("https://").trim_start_matches("http://");
    let h = h.split('/').next().unwrap_or(h).split(':').next().unwrap_or(h);
    EXCHANGE_HOSTS.iter().find(|x| **x == h).copied()
}

/// 从回调里取出 AuthCode。官方回调**不是** `authCode=xxx`，而是
/// `authCodeInfo=<JSON>`（`{"AuthCode":"…"}`）——这是上一版拿不到码、窗口永不关闭的根因。
pub fn auth_code_of(callback: &std::collections::HashMap<String, String>) -> Option<String> {
    for key in ["authCodeInfo", "authCode", "auth_code", "code", "Code"] {
        let Some(raw) = callback.get(key) else { continue };
        let raw = raw.trim().trim_matches('"');
        if raw.is_empty() { continue; }
        if raw.starts_with('{') {
            if let Ok(v) = serde_json::from_str::<Value>(raw) {
                if let Some(s) = pick_str(&v, &["AuthCode", "authCode", "auth_code", "code"]) {
                    if !s.is_empty() { return Some(s); }
                }
            }
            continue;
        }
        return Some(raw.to_string());
    }
    None
}

/// 从回调里取出 userInfo（官方同名字段，JSON 串：UserID/ScreenName/…）
pub fn user_info_of(callback: &std::collections::HashMap<String, String>) -> Value {
    for key in ["userInfo", "user_info"] {
        if let Some(raw) = callback.get(key) {
            let raw = raw.trim().trim_matches('"');
            if raw.starts_with('{') {
                if let Ok(v) = serde_json::from_str::<Value>(raw) { return v; }
            }
        }
    }
    Value::Null
}

/// 官方 `exchangeTokenByAuthCode`：
/// ```text
/// POST {api}/trae/api/v3/oauth/ExchangeToken
/// headers: Content-Type: application/json, x-cloudide-token: ""
/// body: { ClientID, AuthCode, CodeVerifier,
///         DeviceInfo{DeviceID,MachineID,PlatformCode,DeviceType,DeviceName,DeviceModel,
///                    ClientVersion,DevicePublicKey,DeviceBrand,DeviceCPU,OSInfo,OSVersion},
///         IDEVersion }
/// → Result{ Token, RefreshToken, TokenExpireAt, TokenExpireDuration, RefreshExpireAt }
/// ```
/// 换到 token 后必须再打 `GetUserInfo` 拿 `UserID`（真 uid），否则多账号会互相覆盖。
pub async fn exchange_auth_code(login: &TraeLogin, callback: &std::collections::HashMap<String, String>) -> Result<Cred, PlatformError> {
    // 回调里的 error_code 表示被风控/封禁，直接给明确提示
    if let Some(ec) = callback.get("error_code").filter(|s| !s.is_empty()) {
        return Err(PlatformError::Identity(format!(
            "Trae 授权被拒绝（error_code={ec}），该账号可能已被风控，请更换账号或稍后重试"
        )));
    }
    let auth_code = auth_code_of(callback)
        .ok_or_else(|| PlatformError::Format("回调缺少 AuthCode（authCodeInfo/authCode 都没有）".into()))?;

    let meta = crate::adapters::trae_local::product_meta();
    let app_version = meta.app_version.clone().unwrap_or_else(|| APP_VERSION.to_string());
    let client_id = meta.client_id.clone().unwrap_or_else(|| CLIENT_ID_CN.to_string());
    // 换码固定走 API 域：consoleHost(www.trae.cn) 只提供前端页面
    let base = API_BASE_CN.to_string();
    let si = system_information();
    let payload = json!({
        "ClientID": client_id,
        "AuthCode": auth_code,
        "CodeVerifier": login.verifier,
        "DeviceInfo": device_info(&login.identity, &app_version, &si),
        "IDEVersion": app_version,
    });
    let url = format!("{base}{EXCHANGE_PATH}");
    let hs: Vec<(&str, String)> = vec![
        ("Content-Type", "application/json".to_string()),
        ("x-cloudide-token", String::new()),
    ];
    let v = platform_fetch(&url, Method::POST, Some(payload), &hs)
        .await
        .map_err(|e| PlatformError::Network(format!("TRAE 登录换码请求失败: {e}")))?;
    let upstream = v.get("ResponseMetadata").and_then(|m| m.get("Error")).and_then(|e| e.get("Code")).cloned();
    let data = v.get("Result").or_else(|| v.get("data")).cloned().unwrap_or_else(|| v.clone());
    let Some(token) = pick_str(&data, &["Token", "accessToken", "access_token", "token"]) else {
        let msg = v.pointer("/ResponseMetadata/Error/Message").and_then(|m| m.as_str()).unwrap_or("");
        let raw = v.to_string();
        let brief: String = raw.chars().take(320).collect();
        return Err(PlatformError::Format(format!(
            "TRAE 换码失败: code={upstream:?} msg={msg} resp={brief}"
        )));
    };

    // 取用户信息（uid / 昵称 / 区域）——官方 OAuthenticator 换到 token 后必打这一枪
    let mut uid = String::new();
    let mut name = String::new();
    let mut region = String::new();
    let info_hs: Vec<(&str, String)> = vec![
        ("Content-Type", "application/json".to_string()),
        ("x-cloudide-token", token.clone()),
    ];
    let info_body = json!({ "ReqSource": "IDE", "IDEVersion": app_version, "GetUIDFusionInfo": true });
    if let Ok(u) = platform_fetch(&format!("{base}{USERINFO_PATH}"), Method::POST, Some(info_body), &info_hs).await {
        let r = u.get("Result").cloned().unwrap_or(u);
        uid = match r.get("UserID") {
            Some(Value::String(s)) => s.trim().to_string(),
            Some(Value::Number(n)) => n.to_string(),
            _ => String::new(),
        };
        name = r.get("ScreenName").and_then(|s| s.as_str()).unwrap_or("").to_string();
        region = r.get("AIRegion").and_then(|s| s.as_str()).unwrap_or("").to_string();
    }
    // 用户信息拿不到时退回回调里的 userInfo / JWT
    if uid.is_empty() {
        let ui = user_info_of(callback);
        uid = match ui.get("UserID") {
            Some(Value::String(s)) => s.trim().to_string(),
            Some(Value::Number(n)) => n.to_string(),
            _ => String::new(),
        };
        if name.is_empty() {
            name = ui.get("ScreenName").and_then(|s| s.as_str()).unwrap_or("").to_string();
        }
    }
    if uid.is_empty() {
        uid = jwt_uid(&token);
    }
    // 取不到 uid 绝不能入库：accounts 主键是 (provider, uid)，空 uid 会占掉唯一空位，
    // 表现为「第二个账号导入后第一个消失」「同一账号重复登录互相覆盖」。
    if uid.is_empty() {
        return Err(PlatformError::Format(
            "TRAE 换码成功但取不到 UserID（GetUserInfo / userInfo / JWT 全为空），已放弃入库以免覆盖其它账号".into(),
        ));
    }
    if name.is_empty() {
        name = if uid.is_empty() { "Trae 账号".into() } else { format!("Trae {uid}") };
    }

    // 官方 `nUe(TokenExpireAt, TokenExpireDuration)` 用 `new Date(TokenExpireAt)`，
    // 即 epoch 毫秒；缺失时按 TokenExpireDuration（秒）从当前时间推算。
    let expires_at = match data.get("TokenExpireAt") {
        Some(Value::Number(n)) => n.as_i64().map(|ms| iso_from_ms_pub(ms as u64)),
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => data
            .get("TokenExpireDuration")
            .and_then(|d| d.as_i64())
            .filter(|sec| *sec > 0)
            .map(|sec| iso_from_ms_pub(now_ms_pub() + (sec as u64) * 1000)),
    };
    let mut meta_json = json!({
        "deviceId": login.identity.device_id,
        "machineId": login.identity.machine_id,
        "host": base,
        "reqSource": 1,
        "appVersion": app_version,
        "privateKeyPem": login.identity.private_key_pem,
        "publicKeyPem": login.identity.public_key_pem,
        "loginTraceId": login.trace_id,
    });
    if !region.is_empty() {
        if let Some(o) = meta_json.as_object_mut() { o.insert("region".into(), json!(region)); }
    }
    Ok(Cred {
        provider: "trae".into(),
        uid,
        name,
        token,
        refresh_token: pick_str(&data, &["RefreshToken", "refreshToken", "refresh_token"]),
        expires_at,
        meta: meta_json,
    })
}

/// 当前时间（epoch 毫秒）
pub fn now_ms_pub() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 毫秒时间戳 → UTC RFC3339（供登录换码使用；不引 chrono）
pub fn iso_from_ms_pub(ms: u64) -> String {    let total = ms / 1000;
    let days = (total / 86400) as i64;
    let rem = total % 86400;
    let (y, mo, d) = crate::adapters::civil_from_days(days);
    format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
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

/// 轻量 percent-encoding（仅字母数字与 -_.~ 不转义），避免引 URL 库依赖
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

/// 设备品牌（登录 URL 的 x_device_brand；取 COMPUTERNAME）
pub fn device_model() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into())
}

/// 系统版本（x_os_version；取 OS 环境变量）
pub fn os_version() -> String {
    std::env::var("OS").unwrap_or_else(|_| "Windows".into())
}

/// 用户名（DeviceInfo.DeviceName；取 USERNAME）
pub fn username() -> String {
    std::env::var("USERNAME").unwrap_or_else(|_| "user".into())
}

