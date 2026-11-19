//! WorkBuddy/CodeBuddy（腾讯国内版）适配器。
//! headers: Authorization Bearer / X-User-Id / X-Domain / X-Product: SaaS [/ X-Enterprise-Id]
//! 端点：/v2/plugin/account、/v2/billing/meter/{checkin-activity-status,daily-checkin,get-user-resource}、
//! 刷新 copilot.tencent.com/v2/plugin/auth/token/refresh（X-Refresh-Token）。
//! 错误码：12153 会话失效；40901/40100 国际版。

use serde_json::{json, Value};

use crate::adapters::*;
use crate::error::PlatformError;
use crate::net::http::{business, platform_fetch};
use reqwest::Method;

/// WorkBuddy/CodeBuddy 适配器；intl=true 走国际版域名（workbuddy.ai）与 provider 名
pub struct WorkBuddyAdapter {
    pub intl: bool,
}

fn host(c: &Cred) -> String {
    let default = if self_intl(c) { "https://www.workbuddy.ai" } else { "https://www.codebuddy.cn" };
    c.meta.get("domain").and_then(|d| d.as_str())
        .map(|d| if d.starts_with("http") { d.trim_end_matches('/').to_string() } else { format!("https://{d}") })
        .unwrap_or_else(|| default.to_string())
}

fn self_intl(c: &Cred) -> bool {
    c.provider == "workbuddy-intl"
}

fn headers(c: &Cred) -> Vec<(String, String)> {
    let mut hs = vec![
        ("Authorization".into(), format!("Bearer {}", c.token)),
        ("X-User-Id".into(), c.uid.clone()),
        ("X-Domain".into(), c.meta.get("domain").and_then(|d| d.as_str()).unwrap_or("www.codebuddy.cn").to_string()),
        ("X-Product".into(), "SaaS".into()),
    ];
    if let Some(e) = c.meta.get("enterpriseId").and_then(|e| e.as_str()) {
        if !e.is_empty() {
            hs.push(("X-Enterprise-Id".into(), e.to_string()));
            hs.push(("X-Tenant-Id".into(), e.to_string()));
        }
    }
    hs
}

async fn request(c: &Cred, path: &str, body: Option<Value>) -> Result<Value, PlatformError> {
    let url = format!("{}{}", host(c), path);
    let hs_full = headers(c);
    let hs: Vec<(&str, String)> = hs_full.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    platform_fetch(&url, Method::POST, body, &hs).await
}

/// 前置身份强校验：GET /v2/plugin/account（国内）；intl 用 JWT.sub==uid 本地校验 + account 查询
async fn verify(c: &Cred) -> Result<(), PlatformError> {
    let url = format!("{}/v2/plugin/account", host(c));
    let hs_full = headers(c);
    let hs: Vec<(&str, String)> = hs_full.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    let v = platform_fetch(&url, Method::GET, None, &hs).await?;
    let d = business(v).map_err(|e| match &e {
        PlatformError::Business { code, .. } if code.as_i64() == Some(12153) => PlatformError::Auth,
        _ => e,
    })?;
    if let Some(uid) = d.get("uid").and_then(|u| u.as_str()) {
        if uid != c.uid {
            return Err(PlatformError::Identity("uid mismatch".into()));
        }
    }
    Ok(())
}

#[async_trait::async_trait]
impl Adapter for WorkBuddyAdapter {
    fn provider(&self) -> &'static str { if self.intl { "workbuddy-intl" } else { "workbuddy" } }

    /// 校验：先过 /v2/plugin/account 强校验（12153→Auth、uid 不一致→Identity）
    async fn verify(&self, c: &Cred) -> Result<Value, PlatformError> {
        verify(c).await?;
        Ok(json!({ "uid": c.uid }))
    }

    /// 查签到活动状态；intl 字段与国内不同（task_completed/eligible 另算 requires_manual）
    async fn state(&self, c: &Cred) -> Result<CheckinState, PlatformError> {
        let path = if self.intl { "/billing/meter/checkin-activity-status" } else { "/v2/billing/meter/checkin-activity-status" };
        let d = business(request(c, path, Some(json!({}))).await?)?;
        if self.intl {
            let active = d.get("active").and_then(|a| a.as_bool()).unwrap_or(true);
            let checked = d.get("today_checked_in").and_then(|v| v.as_bool()).unwrap_or(false);
            let task = d.get("task_completed").and_then(|v| v.as_bool()).unwrap_or(false);
            let eligible = d.get("eligible").and_then(|v| v.as_bool()).unwrap_or(false);
            return Ok(CheckinState {
                checked,
                claimable: !checked && active,
                requires_manual: Some(!checked && (!task || !eligible)),
                raw: d,
                ..Default::default()
            });
        }
        let active = d.get("active").and_then(|a| a.as_bool()).unwrap_or(true);
        let flag = d.get("today_checked_in").or_else(|| d.get("checked_in"));
        let checked = match flag {
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(n)) => n.as_i64() != Some(0),
            _ if !active => false,
            _ => return Err(PlatformError::Format("cannot parse workbuddy state".into())),
        };
        Ok(CheckinState { checked, claimable: !checked && active, raw: d, ..Default::default() })
    }

    /// 领取每日签到；业务码 40901 → Conditions（活动条件未满足）
    async fn claim(&self, c: &Cred, st: &CheckinState) -> Result<Receipt, PlatformError> {
        verify(c).await?;
        if st.checked {
            return Ok(Receipt { status: "already".into(), amount: None, message: Some("本周期此前已领取".into()) });
        }
        if !st.claimable {
            return Ok(Receipt { status: if st.requires_manual == Some(true) { "needs-action".into() } else { "no-activity".into() }, amount: None, message: None });
        }
        let path = "/v2/billing/meter/daily-checkin";
        let d = business(request(c, path, Some(json!({}))).await?).map_err(|e| match &e {
            PlatformError::Business { code, .. } if code.as_i64() == Some(40901) => PlatformError::Conditions,
            _ => e,
        })?;
        let amount = d.get("credits").and_then(|v| v.as_f64())
            .or_else(|| d.get("today_credit").and_then(|v| v.as_f64()))
            .or_else(|| d.get("amount").and_then(|v| v.as_f64()));
        Ok(Receipt { status: "success".into(), amount, message: None })
    }

    /// 查资源包：Response.Data.Accounts 逐项拆 parts（周期套餐取 CycleCapacity 前缀字段）
    async fn quota(&self, c: &Cred) -> Result<Quota, PlatformError> {
        let path = "/v2/billing/meter/get-user-resource";
        let d = business(request(c, path, Some(json!({}))).await?)?;
        let accounts = d.get("Response").and_then(|r| r.get("Data")).and_then(|x| x.get("Accounts")).and_then(|a| a.as_array())
            .ok_or_else(|| PlatformError::Format("cannot parse workbuddy quota".into()))?;
        let mut parts = Vec::new();
        for p in accounts {
            let end = p.get("CycleEndTime").and_then(|v| v.as_str());
            let deduct = p.get("DeductionEndTime").and_then(|v| v.as_str());
            // DeductionEndTime 比 CycleEndTime 晚超 2 天 → 视为周期性套餐（下面按 CycleCapacity 取数）
            let recurring = match (end, deduct) {
                (Some(e), Some(dd)) => match (chrono_like_secs(e), chrono_like_secs(dd)) {
                    (Ok(ed), Ok(ddm)) => ddm - ed > 2 * 86400,
                    _ => false,
                },
                _ => false,
            };
            let prefix = if recurring { "CycleCapacity" } else { "Capacity" };
            let get = |suffix: &str| p.get(format!("{prefix}{suffix}")).or_else(|| p.get(format!("{prefix}Size{suffix}"))).cloned().unwrap_or(Value::Null);
            let total = get("SizePrecise").as_f64().or_else(|| get("Size").as_f64());
            let used = get("UsedPrecise").as_f64().or_else(|| get("Used").as_f64());
            let remain = get("RemainPrecise").as_f64().or_else(|| get("Remain").as_f64());
            parts.push(json!({
                "id": p.get("AccountId").or_else(|| p.get("ResourceId")).or_else(|| p.get("PackageId")).cloned().unwrap_or(Value::Null),
                "name": p.get("PackageName").or_else(|| p.get("SubProductName")).cloned().unwrap_or(json!("积分包")),
                "total": total, "used": used, "remaining": remain,
                "expiresAt": if recurring { end } else { deduct },
            }));
        }
        let sum: f64 = parts.iter().filter_map(|p| p["remaining"].as_f64()).sum();
        Ok(Quota { remaining: if parts.is_empty() { None } else { Some(sum) }, parts: Value::Array(parts), sampled_at: now_iso() })
    }

    /// 用 refresh_token 到 copilot.tencent.com 刷新端点换新 access/refresh token
    async fn refresh(&self, c: &Cred) -> Result<Cred, PlatformError> {
        let Some(rt) = &c.refresh_token else { return Err(PlatformError::Auth) };
        let url = "https://copilot.tencent.com/v2/plugin/auth/token/refresh";
        let mut hs = vec![
            ("X-Refresh-Token".to_string(), rt.clone()),
            ("X-Auth-Refresh-Source".to_string(), "workbuddy".to_string()),
        ];
        if let Some(e) = c.meta.get("enterpriseId").and_then(|e| e.as_str()) {
            hs.push(("X-Enterprise-Id".to_string(), e.to_string()));
        }
        let hs: Vec<(&str, String)> = hs.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let v = platform_fetch(url, Method::POST, Some(json!({})), &hs).await?;
        let d = business(v)?;
        let mut out = c.clone();
        if let Some(t) = d.get("accessToken").and_then(|t| t.as_str()) { out.token = t.to_string(); }
        if let Some(r) = d.get("refreshToken").and_then(|t| t.as_str()) { out.refresh_token = Some(r.to_string()); }
        Ok(out)
    }
}

/// 极简 ISO8601 解析（返回 epoch 秒；仅支持 Z/无 Z 两种格式，供套餐周期判断）
fn chrono_like_secs(s: &str) -> Result<i64, ()> {
    // 简化 ISO 解析：仅支持 "YYYY-MM-DDTHH:MM:SS(.fff)?Z" 与无 Z
    let (date, time) = s.split_once('T').ok_or(())?;
    let dp: Vec<&str> = date.split('-').collect();
    if dp.len() != 3 { return Err(()); }
    let y: i64 = dp[0].parse().map_err(|_| ())?;
    let mo: i64 = dp[1].parse().map_err(|_| ())?;
    let d: i64 = dp[2].parse().map_err(|_| ())?;
    let time = time.trim_end_matches('Z');
    let tp: Vec<&str> = time.split(':').collect();
    if tp.len() < 2 { return Err(()); }
    let h: i64 = tp[0].parse().map_err(|_| ())?;
    let mi: i64 = tp[1].parse().map_err(|_| ())?;
    let sec: i64 = tp.get(2).and_then(|x| x.split('.').next()).and_then(|x| x.parse().ok()).unwrap_or(0);
    Ok(crate::adapters::days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + sec)
}
