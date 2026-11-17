//! 服务编排：对齐原版 DesktopService.invoke 主路径（直连平台部分）。
//! 流程：读账号 → verify(TOCTOU) → state → 判重(cycle) → claim → 记 history → 更新状态。

use std::sync::Arc;

use serde_json::{json, Value};

use crate::adapters::*;
use crate::db::Store;
use crate::error::PlatformError;

pub struct Service {
    pub store: Arc<Store>,
}

pub struct CheckinOutcome {
    pub provider: String,
    pub uid: String,
    pub status: String, // success | already | failed | pending | needs-action | no-activity
    pub amount: Option<f64>,
    pub message: Option<String>,
}

pub fn adapter_for(provider: &str) -> Option<Box<dyn Adapter>> {
    match provider {
        "trae" => Some(Box::new(super::adapters::trae::TraeAdapter { req_source: 1 })),
        "workbuddy" => Some(Box::new(super::adapters::workbuddy::WorkBuddyAdapter { intl: false })),
        "workbuddy-intl" => Some(Box::new(super::adapters::workbuddy::WorkBuddyAdapter { intl: true })),
        "minimax" => Some(Box::new(super::adapters::minimax::MiniMaxAdapter)),
        "qoder" => Some(Box::new(super::adapters::qoder::QoderAdapter { intl: false })),
        "qoder-intl" => Some(Box::new(super::adapters::qoder::QoderAdapter { intl: true })),
        "codearts" => Some(Box::new(super::adapters::codearts::CodeArtsAdapter { desktop: false })),
        "codearts-desktop" => Some(Box::new(super::adapters::codearts::CodeArtsAdapter { desktop: true })),
        "zcode" => Some(Box::new(super::adapters::zcode::ZcodeAdapter)),
        _ => None,
    }
}

impl Service {
    pub async fn checkin_account(&self, provider: &str, uid: &str) -> Result<CheckinOutcome, PlatformError> {
        let accounts = self.store.list_accounts().map_err(|e| PlatformError::Local(e.to_string()))?;
        let Some((_, _, name, secret_b64, auth)) = accounts.iter().find(|(p, u, _, _, _)| p == provider && u == uid) else {
            return Err(PlatformError::Local(format!("account {provider}:{uid} not found")));
        };
        let cred: Cred = serde_json::from_str(auth).map_err(|e| PlatformError::Local(format!("bad cred json: {e}")))?;
        let _ = secret_b64; // secret 字段保存加密凭据；本轮 cred 明文存 auth 列（单机 DPAPI 由 vault 处理）
        let adapter = adapter_for(provider).ok_or_else(|| PlatformError::Local(format!("no adapter for {provider}")))?;
        let cycle = crate::adapters::day_cn();

        // 当日已成功/已领 → 直接 already（幂等）
        if let Ok(rows) = self.store.history_today(provider, uid, &cycle) {
            if !rows.is_empty() {
                return Ok(CheckinOutcome { provider: provider.into(), uid: uid.into(), status: "already".into(), amount: None, message: Some("今日已完成".into()) });
            }
        }

        let st = adapter.state(&cred).await?;
        let receipt = adapter.claim(&cred, &st).await;
        let outcome = match receipt {
            Ok(r) => CheckinOutcome { provider: provider.into(), uid: uid.into(), status: r.status.clone(), amount: r.amount, message: r.message.clone() },
            Err(e) => CheckinOutcome { provider: provider.into(), uid: uid.into(), status: "failed".into(), amount: None, message: Some(e.to_string()) },
        };
        let _ = self.store.record_history(provider, uid, name, &cycle, &outcome.status, outcome.amount, outcome.message.as_deref(), &crate::adapters::now_iso());
        let _ = self.store.update_account_status(provider, uid, &outcome.status, outcome.message.as_deref());
        Ok(outcome)
    }

    pub async fn quota_account(&self, provider: &str, uid: &str) -> Result<Quota, PlatformError> {
        let accounts = self.store.list_accounts().map_err(|e| PlatformError::Local(e.to_string()))?;
        let Some((_, _, _, _, auth)) = accounts.iter().find(|(p, u, _, _, _)| p == provider && u == uid) else {
            return Err(PlatformError::Local("account not found".into()));
        };
        let cred: Cred = serde_json::from_str(auth).map_err(|e| PlatformError::Local(e.to_string()))?;
        let adapter = adapter_for(provider).ok_or_else(|| PlatformError::Local("no adapter".into()))?;
        let q = adapter.quota(&cred).await?;
        // 额度持久化：下次启动直接展示，无需手动点击
        if let Ok(qj) = serde_json::to_string(&q) {
            let _ = self.store.update_account_quota(provider, uid, &qj);
        }
        Ok(q)
    }

    pub fn state_summary(&self) -> Value {
        let accounts = self.store.list_accounts().unwrap_or_default();
        json!({
            "accounts": accounts.iter().map(|(p, u, n, _, _)| json!({ "provider": p, "uid": u, "name": n })).collect::<Vec<_>>(),
        })
    }

    /// UI 快照：账号全行 + 最近历史。
    pub fn ui_snapshot(&self) -> Value {
        let accounts = self.store.list_accounts_full().unwrap_or_default();
        let history = self.store.list_history(50).unwrap_or_default();
        json!({
            "accounts": accounts,
            "history": history,
            "cycle": crate::adapters::day_cn(),
        })
    }

    /// 本机扫描导入（trae/wb/qoder），对齐 CLI import 逻辑。
    pub async fn import_account(&self, platform: &str, intl: bool) -> Result<Value, PlatformError> {
        let cred = match platform {
            "trae" => {
                let Some(live) = crate::adapters::trae_local::read_live_auth() else {
                    return Err(PlatformError::Local("未发现可读取的 Trae 登录信息（未登录或未安装）".into()));
                };
                if live.is_intl {
                    return Err(PlatformError::Local(format!("检测到国际版登录态（host={}），当前仅支持国内版", live.host.unwrap_or_default())));
                }
                let token = live.auth.get("token").and_then(|t| t.as_str()).unwrap_or_default().to_string();
                if token.is_empty() {
                    return Err(PlatformError::Local("Trae 登录态 token 为空".into()));
                }
                let uid = jwt_uid_of(&token);
                crate::adapters::Cred {
                    provider: "trae".into(),
                    uid,
                    name: format!("Trae ({})", live.data_dir.file_name().unwrap_or_default().to_string_lossy()),
                    token,
                    refresh_token: live.auth.get("refreshToken").and_then(|t| t.as_str()).map(|s| s.to_string()),
                    expires_at: None,
                    meta: json!({
                        "deviceId": live.device_id,
                        "host": live.host.unwrap_or_else(|| "https://api.trae.cn".into()),
                        "reqSource": crate::adapters::trae_local::request_source_for_dir(&live.data_dir),
                    }),
                }
            }
            "wb" | "workbuddy" => {
                let sess = crate::adapters::wb_local::read_session(intl, None).ok().flatten()
                    .ok_or_else(|| PlatformError::Local("未发现 WorkBuddy 会话文件（未登录或未安装）".into()))?;
                let acc = crate::adapters::wb_local::session_to_account(&sess)
                    .ok_or_else(|| PlatformError::Local("会话文件缺少 accessToken".into()))?;
                crate::adapters::Cred {
                    provider: if intl { "workbuddy-intl".into() } else { "workbuddy".into() },
                    uid: acc.get("uid").and_then(|u| u.as_str()).unwrap_or_default().to_string(),
                    name: acc.get("name").and_then(|n| n.as_str()).unwrap_or("WorkBuddy").to_string(),
                    token: acc.get("token").and_then(|t| t.as_str()).unwrap_or_default().to_string(),
                    refresh_token: acc.get("refreshToken").and_then(|t| t.as_str()).map(|s| s.to_string()),
                    expires_at: None,
                    meta: json!({
                        "domain": acc.get("domain").and_then(|d| d.as_str()).unwrap_or(if intl { "www.workbuddy.ai" } else { "www.codebuddy.cn" }),
                        "enterpriseId": acc.get("enterpriseId").and_then(|e| e.as_str()).unwrap_or_default(),
                    }),
                }
            }
            "qoder" => {
                let v = crate::adapters::qoder_local::read_auth(intl).map_err(|e| PlatformError::Local(e.to_string()))?
                    .ok_or_else(|| PlatformError::Local("未发现 Qoder auth.v1.dat（未登录或未安装）".into()))?;
                crate::adapters::Cred {
                    provider: if intl { "qoder-intl".into() } else { "qoder".into() },
                    uid: v.pointer("/user/id").and_then(|u| u.as_str()).map(|s| s.to_string())
                        .or_else(|| v.pointer("/user/id").map(|u| u.to_string().trim_matches('"').to_string()))
                        .unwrap_or_default(),
                    name: v.pointer("/user/name").and_then(|n| n.as_str()).unwrap_or("Qoder").to_string(),
                    token: v.get("token").and_then(|t| t.as_str()).unwrap_or_default().to_string(),
                    refresh_token: None,
                    expires_at: v.get("expiresAt").and_then(|e| e.as_str()).map(|s| s.to_string()),
                    meta: json!({}),
                }
            }
            "zcode" => {
                let Some(live) = crate::adapters::zcode_local::read_live_auth() else {
                    return Err(PlatformError::Local("未发现 ZCode 登录信息（客户端未安装或未登录）".into()));
                };
                crate::adapters::Cred {
                    provider: "zcode".into(),
                    uid: live.uid,
                    name: live.name.unwrap_or_else(|| "ZCode (智谱)".into()),
                    token: live.access_token,
                    refresh_token: None,
                    expires_at: None,
                    meta: json!({ "deviceMid": live.device_mid }),
                }
            }
            other => return Err(PlatformError::Local(format!("platform {other} 暂不支持本机扫描导入（minimax/codearts 走登录流程，doubao 走窗口驱动）"))),
        };
        let auth = serde_json::to_string(&cred).map_err(|e| PlatformError::Local(e.to_string()))?;
        self.store.upsert_account(&cred.provider, &cred.uid, &cred.name, "", &auth)
            .map_err(|e| PlatformError::Local(e.to_string()))?;
        Ok(json!({ "provider": cred.provider, "uid": cred.uid, "name": cred.name }))
    }

/// 统计聚合：period = day|week|month|year，按 provider+账号过滤，返回分桶金额/次数。
pub fn statistics(&self, provider: Option<&str>, uid: Option<&str>, period: &str) -> Value {
    let rows = self.store.list_history(5000).unwrap_or_default();
    let mut buckets: std::collections::BTreeMap<String, (f64, i64, f64)> = Default::default(); // key → (sum, count, max)
    for h in rows {
        if let Some(p) = provider { if h.provider != p { continue; } }
        if let Some(u) = uid { if h.uid != u { continue; } }
        let day = h.at.get(..10).unwrap_or("").to_string();
        let key = match period {
            "day" => day.clone(),
            "week" => {
                let (y, w) = Self::iso_week(&day);
                format!("{y}-W{w:02}")
            }
            "month" => day.get(..7).unwrap_or("").to_string(),
            "year" => day.get(..4).unwrap_or("").to_string(),
            _ => day,
        };
        if key.is_empty() { continue; }
        let e = buckets.entry(key).or_insert((0.0, 0, 0.0));
        e.1 += 1;
        if h.status == "success" { e.0 += h.amount.unwrap_or(0.0); }
        if h.status == "success" { e.2 += h.amount.unwrap_or(0.0); }
    }
    let items: Vec<Value> = buckets.into_iter().map(|(k, (sum, cnt, _))| json!({
        "bucket": k, "sum": sum, "count": cnt,
    })).collect();
    json!({ "period": period, "items": items })
}

/// 按账号×日期的收入矩阵（首页图表用）：days 默认 30。
pub fn daily_income(&self, days: i64) -> Value {
    let rows = self.store.list_history(5000).unwrap_or_default();
    let mut per_day: std::collections::BTreeMap<String, f64> = Default::default();
    let mut per_prov_day: std::collections::BTreeMap<(String, String), f64> = Default::default();
    for h in rows {
        let day = h.at.get(..10).unwrap_or("").to_string();
        if day.is_empty() { continue; }
        if h.status == "success" {
            *per_day.entry(day.clone()).or_insert(0.0) += h.amount.unwrap_or(0.0);
            *per_prov_day.entry((day, h.provider.clone())).or_insert(0.0) += h.amount.unwrap_or(0.0);
        }
    }
    // 只保留最近 days 天
    let today = crate::adapters::day_cn();
    let mut items = Vec::new();
    for i in (0..days).rev() {
        let d = Self::shift_day(&today, -i);
        let mut by_prov = json!({});
        for ((day, p), v) in &per_prov_day {
            if *day == d { by_prov[p] = json!(v); }
        }
        items.push(json!({ "day": d, "income": per_day.get(&d).copied().unwrap_or(0.0), "by_provider": by_prov }));
    }
    json!({ "days": items })
}

fn iso_week(day: &str) -> (i64, u32) {
    // 简化 ISO 周：以 1970-01-01（周四）为基准的 7 天分桶，周一为界
    let parts: Vec<i64> = day.split('-').filter_map(|s| s.parse().ok()).collect();
    if parts.len() != 3 { return (0, 0); }
    let (y, m, d) = (parts[0], parts[1] as u32, parts[2] as u32);
    let days = crate::adapters::days_from_civil(y, m as i64, d as i64);
    // 1970-01-05 是周一
    let since = days - 4; // 1970-01-01 → days=0 → 1月5日=4
    let week = since.div_euclid(7);
    let mut wy = week % 52;
    if wy < 0 { wy += 52; }
    (1970 + week.div_euclid(52), (wy + 1) as u32)
}

fn shift_day(day: &str, delta: i64) -> String {
    let parts: Vec<i64> = day.split('-').filter_map(|s| s.parse().ok()).collect();
    if parts.len() != 3 { return day.to_string(); }
    let (y, m, d) = (parts[0], parts[1] as u32, parts[2] as u32);
    let days = crate::adapters::days_from_civil(y, m as i64, d as i64) + delta;
    let (y2, m2, d2) = crate::adapters::civil_from_days(days);
    format!("{y2:04}-{m2:02}-{d2:02}")
}
}

fn jwt_uid_of(token: &str) -> String {
    use base64::Engine;
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 { return String::new(); }
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[1]) else { return String::new() };
    let v: Value = serde_json::from_slice(&bytes).unwrap_or_default();
    v.pointer("/data/id").and_then(|i| i.as_str()).unwrap_or("").to_string()
}
