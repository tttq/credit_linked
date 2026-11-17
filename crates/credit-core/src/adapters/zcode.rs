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

async fn request(c: &Cred, path: &str, method: Method, body: Option<Value>) -> Result<Value, PlatformError> {
    let hs = vec![
        ("Authorization".to_string(), format!("Bearer {}", c.token)),
        ("Content-Type".to_string(), "application/json".to_string()),
        ("X-Device-Id".to_string(), c.meta.get("deviceMid").and_then(|d| d.as_str()).unwrap_or_default().to_string()),
    ];
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
        let captcha_enabled = cfg.pointer("/configs/captcha/enabled").and_then(|b| b.as_bool()).unwrap_or(true);
        if captcha_enabled {
            return Ok(CheckinState { checked: false, claimable: false, requires_manual: Some(true), raw: json!({ "reason": "captcha" }), ..Default::default() });
        }
        // 活动发现：preview + marketing
        let preview = business(request(c, "/api/v1/zcode-plan/plan/preview", Method::GET, None).await?).unwrap_or(Value::Null);
        let plans: Vec<Value> = preview.get("plans").and_then(|p| p.as_array()).cloned().unwrap_or_default()
            .into_iter().filter(|p| is_free_plan(p)).collect();
        let marketing = business(request(c, "/api/v1/zcode-plan/marketing/deliveries", Method::GET, None).await?).unwrap_or(Value::Null);
        let mut deliveries = Vec::new();
        collect_claim_deliveries(&marketing, &mut deliveries);
        Ok(CheckinState {
            checked: false,
            claimable: !plans.is_empty() || !deliveries.is_empty(),
            raw: json!({ "plans": plans, "deliveries": deliveries }),
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
            let pid = plan.get("plan_id").or_else(|| plan.get("id")).and_then(|v| v.as_str()).unwrap_or_default().to_string();
            if pid.is_empty() { continue; }
            match request(c, "/api/v1/zcode-plan/billing/claim", Method::POST, Some(json!({ "plan_id": pid }))).await {
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
        let arr = balance(c).await?;
        let mut total_units = 0.0; let mut used_units = 0.0;
        if let Some(a) = arr.as_array() {
            for p in a {
                if let Some(w) = p.get("windows").and_then(|w| w.as_array()) {
                    for win in w {
                        total_units += win.get("total_units").and_then(|t| t.as_f64()).unwrap_or(0.0);
                        used_units += win.get("used_units").and_then(|t| t.as_f64()).unwrap_or(0.0);
                    }
                }
            }
        }
        Ok(Quota { remaining: Some(total_units - used_units), parts: json!([{ "name": "zcode", "total": total_units, "used": used_units }]), sampled_at: now_iso() })
    }

    async fn refresh(&self, _c: &Cred) -> Result<Cred, PlatformError> {
        Err(PlatformError::Auth)
    }
}

pub async fn balance(c: &Cred) -> Result<Value, PlatformError> {
    let v = request(c, "/api/v1/zcode-plan/billing/user-plans", Method::GET, None).await?;
    let d = business(v)?;
    Ok(d.get("plans").cloned().unwrap_or(d))
}

fn collect_claim_deliveries(v: &Value, out: &mut Vec<Value>) {
    if let Some(obj) = v.as_object() {
        if obj.get("type").and_then(|t| t.as_str()) == Some("claim_zcode_plan") {
            out.push(v.clone());
        }
        for (_k, sub) in obj {
            collect_claim_deliveries(sub, out);
        }
    } else if let Some(arr) = v.as_array() {
        for sub in arr {
            collect_claim_deliveries(sub, out);
        }
    }
}

fn now_secs_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64
}
