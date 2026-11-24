//! ZCode（智谱）适配器（对齐客户端 3.18.2 逆向）：
//! 活动发现 GET /billing/preview → claim（响应 code/data.plan 判定）→ 余额 /billing/balance。
//! billing/profile 类端点只认 zcodejwttoken（无 exp 的长效会话 JWT）；
//! 短效 OAuth access_token（exp≈1h）可经 GET /api/v1/oauth/profile 的
//! `data.bigmodel.access_token` 动态换新 —— 这是官方客户端的取票通道。
//! 
//! 新增：zcode_client_simulator - 完整模拟官方客户端行为（在 mod.rs 中声明）

use serde_json::{json, Value};

use crate::adapters::*;
use crate::error::PlatformError;
use crate::net::http::{business, platform_fetch};
use reqwest::Method;

/// 业务 API host（zcode.z.ai）
pub const BASE: &str = "https://zcode.z.ai";

/// ZCode（智谱）适配器；billing 类端点专用 zcodejwttoken（见 request/billing_request）
pub struct ZcodeAdapter;

/// 业务请求：plan/billing 类端点只认 zcodejwttoken（OAuth access_token 会 401），
/// 其他端点仍用 OAuth access_token。
async fn request(c: &Cred, path: &str, method: Method, body: Option<Value>) -> Result<Value, PlatformError> {
    request_with(c, path, method, body, false).await
}

async fn billing_request(c: &Cred, path: &str, method: Method, body: Option<Value>) -> Result<Value, PlatformError> {
    request_with(c, path, method, body, true).await
}

async fn request_with(c: &Cred, path: &str, method: Method, body: Option<Value>, billing: bool) -> Result<Value, PlatformError> {
    let token = if billing {
        c.meta.get("zcodeJwt").and_then(|d| d.as_str()).filter(|s| !s.is_empty())
            .unwrap_or(&c.token)
    } else { &c.token };
    let mut hs = vec![
        ("Authorization".to_string(), format!("Bearer {}", token)),
        ("Content-Type".to_string(), "application/json".to_string()),
        // 客户端公共源头发射器（逆向）：claim/balance 请求带版本与平台标识
        ("User-Agent".to_string(), format!("ZCode/{}", crate::adapters::zcode_local::client_version())),
        ("X-ZCode-App-Version".to_string(), crate::adapters::zcode_local::client_version()),
        ("X-Platform".to_string(), "win32-x64".to_string()),
        ("HTTP-Referer".to_string(), "https://zcode.z.ai".to_string()),
        ("X-Os-Category".to_string(), "windows".to_string()),
    ];
    let dev = c.meta.get("deviceMid").and_then(|d| d.as_str()).unwrap_or("");
    if !dev.is_empty() { hs.push(("X-Device-Mid".to_string(), dev.to_string())); }
    let hs: Vec<(&str, String)> = hs.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    platform_fetch(&format!("{BASE}{path}"), method, body, &hs).await
}

#[async_trait::async_trait]
impl Adapter for ZcodeAdapter {
    fn provider(&self) -> &'static str { "zcode" }

    /// 校验凭据有效性：用长效 zcodejwttoken 经 /api/v1/oauth/profile 换新票。
    /// 旧实现用磁盘 access_token 调 /client/configs —— 该 token 只有 1h TTL 且客户端
    /// 不回写刷新，导致「扫描导入成功但 verify 永远 401（一直报登录错误）」。
    /// profile 端点同时校验 jwt 有效性并返回新鲜 bigmodel token（实测 code=0）。
    async fn verify(&self, c: &Cred) -> Result<Value, PlatformError> {
        let tok = fresh_bigmodel_token(c).await?;
        Ok(json!({ "verified": true, "token_source": "oauth/profile", "access_token": tok }))
    }

    /// 查活动：/billing/preview 的 plans + captcha 门（命中则 requires_manual）
    async fn state(&self, c: &Cred) -> Result<CheckinState, PlatformError> {
        // 活动来源（逆向确认）：GET /billing/preview?app_version=…&platform=… 的 data.plans[]
        // （configs.startPlanPreview 只是免费套餐展示配置，可领性以 preview 为准）
        let ver = crate::adapters::zcode_local::client_version();
        let v = billing_request(c, &format!("/api/v1/zcode-plan/billing/preview?app_version={ver}&platform=win32-x64"), Method::GET, None).await?;
        let d = business(v)?;
        let plans: Vec<Value> = d.get("plans").and_then(|p| p.as_array()).cloned().unwrap_or_default();
        
        // captcha 门（仅提示用）：enabled 且 skip_model_request!==true 时 claim 需要验证码
        let cfg = business(request(c, &format!("/api/v1/client/configs?app_version={ver}&platform=win32-x64"), Method::GET, None).await?).unwrap_or(Value::Null);
        let captcha_block = cfg.pointer("/configs/captcha/enabled").and_then(|b| b.as_bool()).unwrap_or(false)
            && !cfg.pointer("/configs/captcha/skip_model_request").and_then(|b| b.as_bool()).unwrap_or(false);
        Ok(CheckinState {
            checked: false,
            claimable: !plans.is_empty() && !captcha_block,
            requires_manual: if !plans.is_empty() && captcha_block { Some(true) } else { None },
            raw: json!({ "plans": plans }),
            ..Default::default()
        })
    }

    /// 逐 plan 领取：code==0 且 data.plan 非空为成功，失败回 already（附下次可领时间）
    async fn claim(&self, c: &Cred, st: &CheckinState) -> Result<Receipt, PlatformError> {
        let plans = st.raw.get("plans").and_then(|p| p.as_array()).cloned().unwrap_or_default();
        if plans.is_empty() {
            return Ok(Receipt { status: "no-activity".into(), amount: None, message: Some("当前暂无可领取的签到活动（该平台此刻没有可领奖励，或今日已领完）".into()) });
        }
        // 客户端语义（逆向）：成功 = code===0 && data.plan 存在；失败 = code!==0，
        // data.message + data.plan.ends_at（下次可领时间）。无幂等键（body 只有 plan_id）。
        let mut last_err: Option<PlatformError> = None;
        for plan in &plans {
            let pid = plan.get("plan_id").or_else(|| plan.get("planId")).or_else(|| plan.get("id"))
                .and_then(|v| v.as_str()).unwrap_or_default().to_string();
            if pid.is_empty() { continue; }
            match billing_request(c, "/api/v1/zcode-plan/billing/claim", Method::POST, Some(json!({ "plan_id": pid }))).await {
                Ok(resp) => {
                    let code = resp.get("code").and_then(|x| x.as_i64())
                        .or_else(|| resp.get("code").and_then(|x| x.as_str()).and_then(|s| s.parse().ok()))
                        .unwrap_or(-1);
                    if code == 0 && !resp.pointer("/data/plan").unwrap_or(&Value::Null).is_null() {
                        let granted = resp.pointer("/data/plan/entitlements")
                            .and_then(|e| e.as_array())
                            .map(|a| a.iter().filter_map(|e| e.get("grant_units").and_then(|g| g.as_f64())).sum::<f64>())
                            .unwrap_or(0.0);
                        return Ok(Receipt {
                            status: "success".into(),
                            amount: if granted > 0.0 { Some(granted) } else { None },
                            message: Some(format!("plan {pid} claimed")),
                        });
                    }
                    // 失败：取 message 与 ends_at（下次可领时间）
                    let msg = resp.pointer("/data/message").and_then(|m| m.as_str())
                        .or_else(|| resp.get("msg").and_then(|m| m.as_str()))
                        .unwrap_or("已领取或不可重复领取").to_string();
                    let ends_at = resp.pointer("/data/plan/ends_at").and_then(|e| e.as_i64());
                    let hint = ends_at.map(|ts| {
                        let (y, mo, d) = crate::adapters::civil_from_days(ts / 86400);
                        let rem = ts % 86400;
                        format!("（下次可领 {y:04}-{mo:02}-{d:02} {:02}:{:02}）", rem / 3600, (rem % 3600) / 60)
                    }).unwrap_or_default();
                    return Ok(Receipt { status: "already".into(), amount: None, message: Some(format!("{msg}{hint}")) });
                }
                Err(e) => { last_err = Some(e); }
            }
        }
        Err(last_err.unwrap_or(PlatformError::Format("no claimable plan".into())))
    }

    /// 查余额：remaining = Σ balances[].remaining_units，逐项拆 parts
    async fn quota(&self, c: &Cred) -> Result<Quota, PlatformError> {
        let d = balance(c).await?;
        // 客户端公式：remaining = Σ balances[].remaining_units；total = Σ total_units
        let mut remaining = 0.0;
        if let Some(bs) = d.get("balances").and_then(|b| b.as_array()) {
            for b in bs {
                remaining += b.get("remaining_units").and_then(|t| t.as_f64()).unwrap_or(0.0);
            }
        }
        let parts = json!(d.get("balances").and_then(|b| b.as_array()).cloned().unwrap_or_default()
            .into_iter().map(|b| json!({
                "name": b.get("show_name").or_else(|| b.get("entitlement_id")).cloned().unwrap_or(json!("zcode")),
                "total": b.get("total_units").cloned().unwrap_or(json!(0)),
                "used": b.get("used_units").cloned().unwrap_or(json!(0)),
                "remaining": b.get("remaining_units").cloned().unwrap_or(json!(0)),
                "expiresAt": b.get("expires_at").cloned().unwrap_or(json!(null)),
            })).collect::<Vec<_>>());
        Ok(Quota { remaining: Some(remaining.max(0.0)), parts, sampled_at: now_iso() })
    }

    /// 「续期」= profile 取票链自检：zcodejwttoken 长效有效（无 exp），磁盘 access_token
    /// 过期不影响任何功能（verify/chat 都走 fresh_bigmodel_token）。jwt 失效时返回 Auth，
    /// 由上层走「拉起客户端 → 扫描本机」自愈链。
    async fn refresh(&self, c: &Cred) -> Result<Cred, PlatformError> {
        let _ = fresh_bigmodel_token(c).await?;
        Ok(c.clone())
    }
}

/// 余额查询（/billing/balance，只认 zcodejwttoken）
pub async fn balance(c: &Cred) -> Result<Value, PlatformError> {
    // 真实端点（逆向）：/billing/balance?app_version=…，响应 { plans, balances }，只认 zcodejwttoken
    let ver = crate::adapters::zcode_local::client_version();
    let v = billing_request(c, &format!("/api/v1/zcode-plan/billing/balance?app_version={ver}"), Method::GET, None).await?;
    let d = business(v)?;
    Ok(d.get("data").cloned().unwrap_or(d))
}

// ===== 会话取票（对话用 access_token 的唯一可靠来源）=====

/// 对话请求头（对齐官方 buildZCodeSourceHeadersFromContext + billing 端实测必需集）
fn chat_headers(token: &str, device_mid: &str) -> Vec<(String, String)> {
    vec![
        ("Authorization".to_string(), format!("Bearer {token}")),
        ("Content-Type".to_string(), "application/json".to_string()),
        ("Accept".to_string(), "application/json".to_string()),
        ("anthropic-version".to_string(), "2023-06-01".to_string()),
        ("User-Agent".to_string(), format!("ZCode/{}", crate::adapters::zcode_local::client_version())),
        ("X-ZCode-App-Version".to_string(), crate::adapters::zcode_local::client_version()),
        ("X-Platform".to_string(), "win32-x64".to_string()),
        ("X-Os-Category".to_string(), "windows".to_string()),
        ("X-Client-Language".to_string(), "zh-CN".to_string()),
        ("X-Client-Timezone".to_string(), "Asia/Shanghai".to_string()),
        ("X-Device-Mid".to_string(), device_mid.to_string()),
    ]
}

/// 用长效 zcodejwttoken 经 /api/v1/oauth/profile 换**新鲜**的 bigmodel access_token。
///
/// 逆向证据（ZCode 3.18.2 asar）：
/// - `userinfoUrl = As({ZCODE_ENV:"production"}, "/api/v1/oauth/profile")`（bigmodel provider runtime config）；
/// - 实测响应 `data.bigmodel.access_token` 为 334 字节新鲜 JWT（客户端每次会话取此票）；
/// - 磁盘 credentials.json 里的 access_token 只有 1 小时 TTL 且客户端不回写刷新——
///   这就是网关「一直报登录错误」的根因（存的票永远过期）。
///
/// zcodejwttoken 本身无 exp（仅 iat/sub/user_id），长效有效（billing/balance 实测 200）。
pub async fn fresh_bigmodel_token(c: &Cred) -> Result<String, PlatformError> {
    let jwt = c.meta.get("zcodeJwt").and_then(|d| d.as_str()).filter(|s| !s.is_empty())
        .ok_or(PlatformError::Auth)?;
    // 取票结果缓存：bigmodel access_token 有 ~1h 有效期，没必要每个对话请求都打一次
    // oauth/profile（既增加延迟，也让「每请求一次 profile」成为频率特征）。
    // 这里按 jwt 指纹缓存 45 分钟（留 15 分钟安全边界），并做**单飞**避免并发重复取票。
    let cache_key = {
        use sha2::{Digest, Sha256};
        let d = Sha256::digest(jwt.as_bytes());
        d.iter().take(16).map(|b| format!("{b:02x}")).collect::<String>()
    };
    {
        use std::collections::HashMap;
        use std::sync::{Mutex, OnceLock};
        static CACHE: OnceLock<Mutex<HashMap<String, (String, std::time::Instant)>>> = OnceLock::new();
        const TTL: std::time::Duration = std::time::Duration::from_secs(45 * 60);
        let map = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        if let Ok(g) = map.lock() {
            if let Some((t, at)) = g.get(&cache_key) {
                if at.elapsed() < TTL {
                    return Ok(t.clone());
                }
            }
        }
    }
    // 单飞：并发请求只让一个去取票，其余等它写缓存
    let single = bigmodel_fetch_lock().lock().await;
    {
        use std::collections::HashMap;
        use std::sync::{Mutex, OnceLock};
        static CACHE: OnceLock<Mutex<HashMap<String, (String, std::time::Instant)>>> = OnceLock::new();
        const TTL: std::time::Duration = std::time::Duration::from_secs(45 * 60);
        let map = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        if let Ok(g) = map.lock() {
            if let Some((t, at)) = g.get(&cache_key) {
                if at.elapsed() < TTL {
                    return Ok(t.clone());
                }
            }
        }
    }
    let tok = fetch_bigmodel_token_uncached(c, jwt, c.meta.get("deviceMid").and_then(|d| d.as_str()).unwrap_or("")).await?;
    {
        use std::collections::HashMap;
        use std::sync::{Mutex, OnceLock};
        static CACHE: OnceLock<Mutex<HashMap<String, (String, std::time::Instant)>>> = OnceLock::new();
        let map = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        if let Ok(mut g) = map.lock() {
            g.insert(cache_key, (tok.clone(), std::time::Instant::now()));
        }
    }
    drop(single);
    Ok(tok)
}

/// 服务端下发的模型（`builtinModels`，已按账号权益过滤）
#[derive(Debug, Clone)]
pub struct ZcodeBuiltinModel {
    pub model_id: String,
    pub name: String,
    pub context_window: Option<u64>,
    pub max_completion_tokens: Option<u64>,
    pub supports_images: Option<bool>,
}

/// 拉取服务端 builtin 模型列表（`/api/v1/client/configs` → `data.builtinModels`）。
///
/// 该列表由服务端**按账号权益**下发（无权益模型不出现），比客户端硬编码清单准确，
/// 可避免调用方选到无权益模型而反复吃 `3006`。
pub async fn fetch_builtin_models(c: &Cred) -> Result<Vec<ZcodeBuiltinModel>, PlatformError> {
    let jwt = c.meta.get("zcodeJwt").and_then(|d| d.as_str()).filter(|s| !s.is_empty())
        .ok_or(PlatformError::Auth)?;
    let mid = c.meta.get("deviceMid").and_then(|d| d.as_str()).unwrap_or("");
    let ver = crate::adapters::zcode_local::client_version();
    let url = format!("{BASE}/api/v1/client/configs?app_version={ver}&platform=win32-x64");
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(12)).build()
        .map_err(|e| PlatformError::Network(e.to_string()))?;
    let resp = client.get(&url)
        .header("Authorization", format!("Bearer {jwt}"))
        .header("User-Agent", format!("ZCode/{}", crate::adapters::zcode_local::client_version()))
        .header("X-ZCode-App-Version", crate::adapters::zcode_local::client_version())
        .header("X-Device-Mid", mid)
        .send().await.map_err(|e| PlatformError::Network(e.to_string()))?;
    let st = resp.status().as_u16();
    if st != 200 {
        return Err(if st == 401 { PlatformError::Auth } else {
            PlatformError::Format(format!("client/configs HTTP {st}"))
        });
    }
    let v: Value = resp.json().await.map_err(|e| PlatformError::Format(e.to_string()))?;
    let arr = v.pointer("/data/builtinModels").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    let mut out = Vec::new();
    for m in arr {
        let id = m.get("modelId").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
        if id.is_empty() {
            continue;
        }
        let name = m.get("name").and_then(|x| x.as_str()).unwrap_or(&id).to_string();
        // 上下文窗口：优先 contextWindow，其次 context_window
        let ctx = m.get("contextWindow").or_else(|| m.get("context_window")).and_then(|x| x.as_u64());
        let maxc = m.get("maxCompletionTokens").or_else(|| m.get("max_completion_tokens")).and_then(|x| x.as_u64());
        let vision = m.pointer("/capabilities/vision").and_then(|x| x.as_bool())
            .or_else(|| m.pointer("/modalities/input").and_then(|x| x.as_array())
                .map(|a| a.iter().any(|v| v.as_str() == Some("image"))));
        out.push(ZcodeBuiltinModel {
            model_id: id,
            name,
            context_window: ctx,
            max_completion_tokens: maxc,
            supports_images: vision,
        });
    }
    Ok(out)
}

/// ZCode 服务端的「请求验证」配置（官方 `/api/v1/client/configs` 下发）。
///
/// 逆向依据（app.asar `out/host/index.js`）：
/// - 端点：`GET {BASE}/api/v1/client/configs?app_version=&platform=`，头 `Authorization: <zcodejwttoken>`
/// - 判定：`requiresRequestVerification = provider.mode==="start-plan"
///   && captcha.enabled !== false && captcha.skipModelRequest !== true`
/// - 客户端缓存 1 小时（服务端建议刷新 300s）
#[derive(Debug, Clone, Default)]
pub struct ZcodeCaptchaConfig {
    /// 验证码总开关
    pub enabled: bool,
    /// **关键**：true = 模型请求跳过验证码（直连安全）；false = 必须带验证码头
    pub skip_model_request: bool,
    pub region: Option<String>,
    pub prefix: Option<String>,
    pub scene_id: Option<String>,
}

impl ZcodeCaptchaConfig {
    /// 模型直连是否需要验证码头
    pub fn needs_captcha_for_model(&self) -> bool {
        self.enabled && !self.skip_model_request
    }
}

/// 拉取服务端请求验证配置（失败返回 Err；调用方按「未知」保守处理）
pub async fn fetch_client_captcha_config(c: &Cred) -> Result<ZcodeCaptchaConfig, PlatformError> {
    let jwt = c.meta.get("zcodeJwt").and_then(|d| d.as_str()).filter(|s| !s.is_empty())
        .ok_or(PlatformError::Auth)?;
    let mid = c.meta.get("deviceMid").and_then(|d| d.as_str()).unwrap_or("");
    let ver = crate::adapters::zcode_local::client_version();
    let url = format!("{BASE}/api/v1/client/configs?app_version={ver}&platform=win32-x64");
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(12)).build()
        .map_err(|e| PlatformError::Network(e.to_string()))?;
    let resp = client.get(&url)
        .header("Authorization", format!("Bearer {jwt}"))
        .header("User-Agent", format!("ZCode/{}", crate::adapters::zcode_local::client_version()))
        .header("X-ZCode-App-Version", crate::adapters::zcode_local::client_version())
        .header("X-Device-Mid", mid)
        .send().await.map_err(|e| PlatformError::Network(e.to_string()))?;
    let st = resp.status().as_u16();
    if st != 200 {
        return Err(if st == 401 { PlatformError::Auth } else {
            PlatformError::Format(format!("client/configs HTTP {st}"))
        });
    }
    let v: Value = resp.json().await.map_err(|e| PlatformError::Format(e.to_string()))?;
    let captcha = v.pointer("/data/configs/captcha").cloned().unwrap_or(Value::Null);
    let b = |k_snake: &str, k_camel: &str| {
        captcha.get(k_snake).or_else(|| captcha.get(k_camel)).and_then(|x| x.as_bool())
    };
    let s = |k: &str| captcha.get(k).and_then(|x| x.as_str()).map(|s| s.to_string());
    Ok(ZcodeCaptchaConfig {
        enabled: b("enabled", "enabled").unwrap_or(false),
        skip_model_request: b("skip_model_request", "skipModelRequest").unwrap_or(false),
        region: s("region"),
        prefix: s("prefix"),
        scene_id: s("sceneId").or_else(|| s("scene_id")),
    })
}

/// 带缓存 + 单飞的「模型请求是否需要验证码」查询。
///
/// 返回 `Some(true)` = 需要（直连会被判异常）；`Some(false)` = 不需要；
/// `None` = 查询失败（未知，调用方不该因此阻塞正常请求）。
pub async fn model_request_needs_captcha(c: &Cred) -> Option<bool> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    /// 服务端建议 300s 刷新；这里取 5 分钟
    const TTL: std::time::Duration = std::time::Duration::from_secs(300);
    static CACHE: OnceLock<Mutex<HashMap<String, (bool, std::time::Instant)>>> = OnceLock::new();
    let key = c.meta.get("zcodeJwt").and_then(|d| d.as_str()).unwrap_or("").chars().take(24).collect::<String>();
    let map = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(g) = map.lock() {
        if let Some((v, at)) = g.get(&key) {
            if at.elapsed() < TTL {
                return Some(*v);
            }
        }
    }
    let single = captcha_probe_lock().lock().await;
    if let Ok(g) = map.lock() {
        if let Some((v, at)) = g.get(&key) {
            if at.elapsed() < TTL {
                return Some(*v);
            }
        }
    }
    let out = match fetch_client_captcha_config(c).await {
        Ok(cfg) => {
            let needs = cfg.needs_captcha_for_model();
            if let Ok(mut g) = map.lock() {
                g.insert(key, (needs, std::time::Instant::now()));
            }
            Some(needs)
        }
        Err(_) => None,
    };
    drop(single);
    out
}

/// 验证配置查询单飞锁
fn captcha_probe_lock() -> &'static tokio::sync::Mutex<()> {
    static L: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    L.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn bigmodel_fetch_lock() -> &'static tokio::sync::Mutex<()> {
    static L: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    L.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// 实际取票（无缓存）
async fn fetch_bigmodel_token_uncached(c: &Cred, jwt: &str, mid: &str) -> Result<String, PlatformError> {
    let _ = c;
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(15)).build().unwrap();
    let mut req = client.get(format!("{BASE}/api/v1/oauth/profile"));
    for (k, v) in chat_headers(jwt, mid) {
        if k != "Content-Type" && k != "Accept" && k != "anthropic-version" {
            req = req.header(k, v);
        }
    }
    let resp = req.send().await.map_err(|e| PlatformError::Network(e.to_string()))?;
    let st = resp.status().as_u16();
    if st != 200 {
        return Err(if st == 401 { PlatformError::Auth } else {
            PlatformError::Format(format!("oauth/profile HTTP {st}"))
        });
    }
    let v: Value = resp.json().await.map_err(|e| PlatformError::Format(e.to_string()))?;
    let tok = v.pointer("/data/bigmodel/access_token").and_then(|t| t.as_str()).unwrap_or("").to_string();
    if tok.is_empty() {
        return Err(PlatformError::Format("oauth/profile 未返回 bigmodel access_token（账号未关联 bigmodel？）".into()));
    }
    Ok(tok)
}
