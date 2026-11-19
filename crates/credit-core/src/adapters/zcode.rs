//! ZCode（智谱）适配器（对齐客户端 3.18.2 逆向）：
//! 活动发现 GET /billing/preview → claim（响应 code/data.plan 判定）→ 余额 /billing/balance。
//! billing 类端点只认 zcodejwttoken；无 refresh API（续期 = 重新 OAuth）。

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
        ("User-Agent".to_string(), "ZCode/3.18.2".to_string()),
        ("X-ZCode-App-Version".to_string(), "3.18.2".to_string()),
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

    /// 校验 token：GET /client/configs（走 OAuth access_token）
    async fn verify(&self, c: &Cred) -> Result<Value, PlatformError> {
        let d = business(request(c, "/api/v1/client/configs", Method::GET, None).await?)?;
        Ok(d)
    }

    /// 查活动：/billing/preview 的 plans + captcha 门（命中则 requires_manual）
    async fn state(&self, c: &Cred) -> Result<CheckinState, PlatformError> {
        // 活动来源（逆向确认）：GET /billing/preview?app_version=…&platform=… 的 data.plans[]
        // （configs.startPlanPreview 只是免费套餐展示配置，可领性以 preview 为准）
        let v = billing_request(c, "/api/v1/zcode-plan/billing/preview?app_version=3.18.2&platform=win32-x64", Method::GET, None).await?;
        let d = business(v)?;
        let plans: Vec<Value> = d.get("plans").and_then(|p| p.as_array()).cloned().unwrap_or_default();
        // captcha 门（仅提示用）：enabled 且 skip_model_request!==true 时 claim 需要验证码
        let cfg = business(request(c, "/api/v1/client/configs?app_version=3.18.2&platform=win32-x64", Method::GET, None).await?).unwrap_or(Value::Null);
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
            return Ok(Receipt { status: "no-activity".into(), amount: None, message: None });
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

    /// 不支持续期：token 过期只能重新 OAuth 授权
    async fn refresh(&self, _c: &Cred) -> Result<Cred, PlatformError> {
        Err(PlatformError::Auth)
    }
}

/// 余额查询（/billing/balance，只认 zcodejwttoken）
pub async fn balance(c: &Cred) -> Result<Value, PlatformError> {
    // 真实端点（逆向）：/billing/balance?app_version=…，响应 { plans, balances }，只认 zcodejwttoken
    let v = billing_request(c, "/api/v1/zcode-plan/billing/balance?app_version=3.18.2", Method::GET, None).await?;
    let d = business(v)?;
    Ok(d.get("data").cloned().unwrap_or(d))
}
