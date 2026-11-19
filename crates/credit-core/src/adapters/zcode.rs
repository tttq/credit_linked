//! ZCode（智谱）适配器：活动发现 → captcha 配置门 → before 快照 → claim →
//! after 快照 → correlate 五条件唯一匹配（匹配不上 pending，永不重发）。

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::adapters::*;
use crate::error::PlatformError;
use crate::net::http::{business, platform_fetch};
use reqwest::Method;

pub const BASE: &str = "https://zcode.z.ai";

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
    ];
    let dev = c.meta.get("deviceMid").and_then(|d| d.as_str()).unwrap_or("");
    if !dev.is_empty() { hs.push(("X-Device-Mid".to_string(), dev.to_string())); }
    let hs: Vec<(&str, String)> = hs.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    platform_fetch(&format!("{BASE}{path}"), method, body, &hs).await
}

/// 活动证据键：SHA256(JSON([planId, [[entitlement_id, effective_at], ...]]))
pub fn evidence_key(plan_id: &str, entitlements: &[Value]) -> String {
    let pairs: Vec<Value> = entitlements.iter().map(|e| json!([
        e.get("entitlement_id").cloned().unwrap_or(Value::Null),
        e.get("effective_at").cloned().unwrap_or(Value::Null),
    ])).collect();
    let payload = json!([plan_id, pairs]);
    hex_lower(&Sha256::digest(payload.to_string().as_bytes()))
}

pub fn hex_lower(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02x}")).collect() }

/// 免费活动判定：无 price/amount/payment/subscription/invitation_code/conditions，
/// requires_captcha !== true，claimable !== false
fn is_free_plan(p: &Value) -> bool {
    for k in ["price", "amount", "payment", "subscription", "invitation_code", "conditions"] {
        if p.get(k).map(|v| !v.is_null()).unwrap_or(false) { return false; }
    }
    if p.get("requires_captcha").and_then(|v| v.as_bool()) == Some(true) { return false; }
    if p.get("claimable").and_then(|v| v.as_bool()) == Some(false) { return false; }
    true
}

#[async_trait::async_trait]
impl Adapter for ZcodeAdapter {
    fn provider(&self) -> &'static str { "zcode" }

    async fn verify(&self, c: &Cred) -> Result<Value, PlatformError> {
        let d = business(request(c, "/api/v1/client/configs", Method::GET, None).await?)?;
        Ok(d)
    }

    async fn state(&self, c: &Cred) -> Result<CheckinState, PlatformError> {
        let cfg = business(request(c, "/api/v1/client/configs", Method::GET, None).await?)?;
        // 活动来自 config：startPlanPreview（免费套餐预览）+ codingPlanStaticProducts
        let mut plans: Vec<Value> = Vec::new();
        if let Some(mut p) = cfg.pointer("/configs/startPlanPreview").cloned() {
            if p.get("plan_id").is_none() { p["plan_id"] = p.get("planId").cloned().unwrap_or(Value::Null); }
            // startPlanPreview 即免费体验套餐，直接纳入（claim 阶段再按 plan_id 领取）
            plans.push(p);
        }
        if let Some(arr) = cfg.pointer("/configs/codingPlanStaticProducts").and_then(|x| x.as_array()) {
            for p in arr {
                let mut v = p.clone();
                if v.get("plan_id").is_none() { v["plan_id"] = v.get("planId").cloned().unwrap_or(Value::Null); }
                if is_free_plan(&v) { plans.push(v); }
            }
        }
        // 兼容旧端点（若服务端仍提供）
        if plans.is_empty() {
            if let Ok(preview) = billing_request(c, "/api/v1/zcode-plan/plan/preview", Method::GET, None).await {
                if let Ok(d) = business(preview) {
                    let mut found: Vec<Value> = d.get("plans").and_then(|p| p.as_array()).cloned().unwrap_or_default()
                        .into_iter().filter(is_free_plan).collect();
                    plans.append(&mut found);
                }
            }
        }
        let captcha_manual = cfg.pointer("/configs/captcha/enabled").and_then(|b| b.as_bool()).unwrap_or(false)
            && cfg.pointer("/configs/captcha/skip_model_request").and_then(|b| b.as_bool()).unwrap_or(false) == false;
        Ok(CheckinState {
            checked: false,
            claimable: !plans.is_empty(),
            requires_manual: if plans.is_empty() && captcha_manual { Some(true) } else { None },
            raw: json!({ "plans": plans }),
            ..Default::default()
        })
    }

    async fn claim(&self, c: &Cred, st: &CheckinState) -> Result<Receipt, PlatformError> {
        let plans = st.raw.get("plans").and_then(|p| p.as_array()).cloned().unwrap_or_default();
        if plans.is_empty() {
            return Ok(Receipt { status: "no-activity".into(), amount: None, message: None });
        }
        // before 快照
        let before = balance(c).await?;
        let before_ids: Vec<Value> = before.as_array().map(|a| a.iter().map(|p| p.get("user_plan_id").cloned().unwrap_or(Value::Null)).collect()).unwrap_or_default();
        let mut last_err: Option<PlatformError> = None;
        for plan in &plans {
            let pid = plan.get("plan_id").or_else(|| plan.get("planId")).or_else(|| plan.get("id"))
                .and_then(|v| v.as_str()).unwrap_or_default().to_string();
            if pid.is_empty() { continue; }
            match billing_request(c, "/api/v1/zcode-plan/billing/claim", Method::POST, Some(json!({ "plan_id": pid }))).await {
                Ok(resp) => {
                    let direct = resp.pointer("/data/plan/user_plan_id").cloned().unwrap_or(Value::Null);
                    if !direct.is_null() {
                        return Ok(Receipt { status: "success".into(), amount: None, message: Some(format!("plan {pid} claimed")) });
                    }
                    // correlate：after 快照唯一匹配
                    let after = balance(c).await?;
                    let anchor = now_secs_ms();
                    let matched = after.as_array().map(|arr| {
                        let cands: Vec<&Value> = arr.iter().filter(|p| {
                            let same_plan = p.get("plan_id").and_then(|v| v.as_str()) == Some(pid.as_str());
                            let new_id = !before_ids.contains(&p.get("user_plan_id").cloned().unwrap_or(Value::Null));
                            let starts = p.get("starts_at").and_then(|v| v.as_f64()).unwrap_or(0.0);
                            let in_window = starts >= (anchor - 5000) as f64 && starts <= (anchor + 120000) as f64;
                            same_plan && new_id && in_window
                        }).collect();
                        cands.len() == 1
                    }).unwrap_or(false);
                    if matched {
                        return Ok(Receipt { status: "success".into(), amount: None, message: Some(format!("plan {pid} correlated")) });
                    }
                    return Ok(Receipt { status: "pending".into(), amount: None, message: Some(format!("plan {pid} submitted, awaiting correlation")) });
                }
                Err(e) => { last_err = Some(e); }
            }
        }
        Err(last_err.unwrap_or(PlatformError::Format("no claimable plan".into())))
    }

    async fn quota(&self, c: &Cred) -> Result<Quota, PlatformError> {
        let d = balance(c).await?;
        let mut total_units = 0.0; let mut used_units = 0.0;
        // 真实端点 /billing/balance 返回 { plans:[...], balances:[...] }
        if let Some(bs) = d.get("balances").and_then(|b| b.as_array()) {
            for b in bs {
                total_units += b.get("total_units").or_else(|| b.get("total")).and_then(|t| t.as_f64()).unwrap_or(0.0);
                used_units += b.get("used_units").or_else(|| b.get("used")).and_then(|t| t.as_f64()).unwrap_or(0.0);
            }
        }
        if total_units == 0.0 {
            if let Some(ps) = d.as_array().or_else(|| d.get("plans").and_then(|p| p.as_array())) {
                for p in ps {
                    if let Some(ents) = p.get("entitlements").and_then(|e| e.as_array()) {
                        for e in ents {
                            total_units += e.get("grant_units").and_then(|t| t.as_f64()).unwrap_or(0.0);
                        }
                    }
                    if let Some(w) = p.get("windows").and_then(|w| w.as_array()) {
                        for win in w {
                            total_units += win.get("total_units").and_then(|t| t.as_f64()).unwrap_or(0.0);
                            used_units += win.get("used_units").and_then(|t| t.as_f64()).unwrap_or(0.0);
                        }
                    }
                }
            }
        }
        Ok(Quota { remaining: Some((total_units - used_units).max(0.0)), parts: json!([{ "name": "zcode", "total": total_units, "used": used_units }]), sampled_at: now_iso() })
    }

    async fn refresh(&self, _c: &Cred) -> Result<Cred, PlatformError> {
        Err(PlatformError::Auth)
    }
}

pub async fn balance(c: &Cred) -> Result<Value, PlatformError> {
    // 真实端点：/billing/balance（旧的 user-plans 实测 404）
    let v = billing_request(c, "/api/v1/zcode-plan/billing/balance", Method::GET, None).await?;
    let d = business(v)?;
    Ok(d.get("data").cloned().unwrap_or(d))
}

fn now_secs_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
}
