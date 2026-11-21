//! Qoder 适配器（国内 openapi.qoder.com.cn / 国际 openapi.qoder.sh）。
//! headers: Authorization Bearer / Cosy-ClientType: "10" / Cosy-Machine* 设备组。
//! 端点：/sash/api/v1/me/campaigns、/sash/api/v1/me/campaigns/{id}/claim、/api/v2/quota/usage。

use serde_json::{json, Value};

use crate::adapters::*;
use crate::error::PlatformError;
use crate::net::http::{business, platform_fetch};
use reqwest::Method;

/// Qoder 适配器；intl=true 走 openapi.qoder.sh（国际），否则 .com.cn（国内）
pub struct QoderAdapter { pub intl: bool }

fn host(intl: bool) -> &'static str { if intl { "https://openapi.qoder.sh" } else { "https://openapi.qoder.com.cn" } }

fn headers(c: &Cred) -> Vec<(String, String)> {
    let mut hs = vec![
        ("Authorization".into(), format!("Bearer {}", c.token)),
        ("Cosy-ClientType".into(), "10".into()),
    ];
    let m = &c.meta;
    for (key, header) in [
        ("machineId", "Cosy-MachineId"),
        ("machineToken", "Cosy-MachineToken"),
        ("machineCode", "Cosy-MachineCode"),
        ("machineType", "Cosy-MachineType"),
        ("clientVersion", "Cosy-Version"),
        ("machineOs", "Cosy-MachineOS"),
    ] {
        if let Some(v) = m.get(key).and_then(|x| x.as_str()) {
            hs.push((header.into(), v.to_string()));
        }
    }
    hs
}

async fn request(c: &Cred, path: &str, method: Method, body: Option<Value>) -> Result<Value, PlatformError> {
    let url = format!("{}{}", host(c.provider == "qoder-intl"), path);
    let hs_full = headers(c);
    let hs: Vec<(&str, String)> = hs_full.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    platform_fetch(&url, method, body, &hs).await
}

#[async_trait::async_trait]
impl Adapter for QoderAdapter {
    fn provider(&self) -> &'static str { if self.intl { "qoder-intl" } else { "qoder" } }

    /// 校验 token：GET /api/v1/userinfo；uid 与凭据不一致 → Identity
    async fn verify(&self, c: &Cred) -> Result<Value, PlatformError> {
        let d = business(request(c, "/api/v1/userinfo", Method::GET, None).await?)?;
        let uid = d.get("id").or_else(|| d.get("userId")).or_else(|| d.get("user_id")).and_then(|u| u.to_string().trim_matches('"').to_string().into());
        if let Some(u) = &uid {
            if u != &c.uid && !u.is_empty() {
                return Err(PlatformError::Identity("qoder uid mismatch".into()));
            }
        }
        Ok(d)
    }

    /// 查活动：campaigns 按时间窗过滤；可领 = CLAIM_BENEFIT+CLAIMABLE，全部 CLAIMED 即已签
    async fn state(&self, c: &Cred) -> Result<CheckinState, PlatformError> {
        let d = business(request(c, "/sash/api/v1/me/campaigns?clientType=10&forceRefresh=true", Method::GET, None).await?)?;
        let campaigns = d.get("campaigns").and_then(|x| x.as_array()).cloned().unwrap_or_default();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as f64;
        let active: Vec<&Value> = campaigns.iter().filter(|x| {
            let s = x.get("startAt").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let e = x.get("endAt").and_then(|v| v.as_f64()).unwrap_or(f64::MAX);
            (s == 0.0 || s <= now) && (e > now)
        }).collect();
        let claimable: Vec<Value> = active.iter().filter(|x| {
            x.get("actionType").and_then(|a| a.as_str()) == Some("CLAIM_BENEFIT")
                && x.get("claimStatus").and_then(|a| a.as_str()) == Some("CLAIMABLE")
        }).map(|x| (*x).clone()).collect();
        let benefits: Vec<&Value> = active.iter().filter(|x| x.get("actionType").and_then(|a| a.as_str()) == Some("CLAIM_BENEFIT")).copied().collect();
        let checked = !benefits.is_empty() && benefits.iter().all(|x| x.get("claimStatus").and_then(|a| a.as_str()) == Some("CLAIMED"));
        Ok(CheckinState {
            checked,
            claimable: !claimable.is_empty(),
            raw: json!({ "campaigns": claimable }),
            ..Default::default()
        })
    }

    /// 逐活动领奖；回执 status 非 CLAIMED/空 → failed（透出平台文案）
    async fn claim(&self, c: &Cred, st: &CheckinState) -> Result<Receipt, PlatformError> {
        if st.checked {
            return Ok(Receipt { status: "already".into(), amount: None, message: Some("本周期此前已领取".into()) });
        }
        let campaigns = st.raw.get("campaigns").and_then(|x| x.as_array()).cloned().unwrap_or_default();
        if campaigns.is_empty() {
            return Ok(Receipt { status: "no-activity".into(), amount: None, message: Some("当前暂无可领取的签到活动（该平台此刻没有可领奖励，或今日已领完）".into()) });
        }
        let mut amount: Option<f64> = None;
        for camp in &campaigns {
            let id = camp.get("campaignId").or_else(|| camp.get("id")).and_then(|v| v.as_str())
                .ok_or_else(|| PlatformError::Format("campaign missing id".into()))?;
            let d = business(request(c, &format!("/sash/api/v1/me/campaigns/{}/claim", crate::adapters::trae::urlencoding_lite(id)), Method::POST, Some(json!({}))).await?)?;
            let status = d.get("status").and_then(|s| s.as_str()).unwrap_or("");
            match status {
                // 徽收状态：上游可能回 CLAIMED / SUCCESS / OK，也可能不回 status（空）
                "CLAIMED" | "SUCCESS" | "OK" | "" => {
                    // 关键修复：上游**不返回** `grantedAmount`，真实金额在活动自身的
                    // `benefit.amount`（实测「每天领 100 Credits」→ amount=100）。
                    // 旧实现只读 grantedAmount，导致回执 amount=0，UI 看起来像没领到。
                    let granted = d.get("grantedAmount").and_then(|g| g.as_f64())
                        .or_else(|| d.pointer("/benefit/amount").and_then(|g| g.as_f64()))
                        .or_else(|| camp.pointer("/benefit/amount").and_then(|g| g.as_f64()))
                        .unwrap_or(0.0);
                    amount = Some(amount.unwrap_or(0.0) + granted);
                }
                other => return Ok(Receipt { status: "failed".into(), amount: None, message: Some(other.to_string()) }),
            }
        }
        Ok(Receipt { status: "success".into(), amount, message: None })
    }

    /// 查额度：userQuota/addOnQuota/orgResourcePackage 三个来源各拆一个 part
    async fn quota(&self, c: &Cred) -> Result<Quota, PlatformError> {
        let d = business(request(c, "/api/v2/quota/usage", Method::GET, None).await?)?;
        let mut parts = Vec::new();
        for (key, name) in [("userQuota", "套餐额度"), ("addOnQuota", "附加额度"), ("orgResourcePackage", "组织资源包")] {
            if let Some(p) = d.get(key) {
                if !p.is_null() {
                    parts.push(json!({
                        "id": "", "name": name,
                        "total": p.get("total"), "used": p.get("used"),
                        "remaining": p.get("remaining"),
                        "expiresAt": p.get("expiresAt").or_else(|| d.get("expiresAt")).cloned().unwrap_or(Value::Null),
                    }));
                }
            }
        }
        let sum: f64 = parts.iter().filter_map(|p| p["remaining"].as_f64()).sum();
        Ok(Quota { remaining: if parts.is_empty() { None } else { Some(sum) }, parts: Value::Array(parts), sampled_at: now_iso() })
    }

    async fn refresh(&self, _c: &Cred) -> Result<Cred, PlatformError> {
        // Qoder token 长期有效，原版无独立 refresh 端点；过期走重新授权
        Err(PlatformError::Auth)
    }
}
