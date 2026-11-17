//! 账号路由：凭据解析 + 平台选择 + 故障自动换号。
//!
//! 路由语义：
//! - 请求 model = "平台/模型" → 显式平台
//! - 请求 model = 裸 id → 当前平台（settings 表 gw_platform 持久化）
//! - 平台内多账号：非 failed 账号优先；请求失败（AUTH/RATE）自动 failover 到下一个账号
//! - 路由规则（gw_rules）：按模型前缀把请求固定到指定平台/账号，UI 可管理

use std::sync::Arc;

use credit_core::adapters::Cred;
use credit_core::db::Store;

use crate::protocol::Platform;

/// 一条路由规则：把匹配的请求固定到指定账号
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RouteRule {
    pub id: String,
    /// 匹配模型前缀；"*" 匹配全部
    pub model_pattern: String,
    /// 目标平台（Platform::as_str）；空 = 不限
    pub platform: String,
    /// 目标账号 provider:uid；空 = 平台默认选号
    pub account: String,
    /// 权重（预留：同前缀多条规则时按权重选）
    pub weight: u32,
    pub enabled: bool,
    pub created_at: String,
}

pub struct Router {
    pub store: Arc<Store>,
}

impl Router {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    // ---------- 当前平台 ----------

    pub fn current_platform(&self) -> Platform {
        let v = self.store.get_setting("gw_platform", "workbuddy").unwrap_or_default();
        Platform::parse(&v).unwrap_or(Platform::WorkBuddy)
    }

    pub fn set_current_platform(&self, p: Platform) {
        let _ = self.store.set_setting("gw_platform", p.as_str());
    }

    // ---------- 路由规则 ----------

    pub fn list_rules(&self) -> Vec<RouteRule> {
        let raw = self.store.get_setting("gw_rules", "[]").unwrap_or_else(|| "[]".into());
        serde_json::from_str(&raw).unwrap_or_default()
    }

    fn save_rules(&self, rules: &[RouteRule]) {
        if let Ok(s) = serde_json::to_string(rules) {
            let _ = self.store.set_setting("gw_rules", &s);
        }
    }

    pub fn add_rule(&self, mut rule: RouteRule) -> RouteRule {
        rule.id = uuid::Uuid::new_v4().to_string();
        rule.created_at = credit_core::adapters::now_iso();
        let mut rules = self.list_rules();
        rules.push(rule.clone());
        self.save_rules(&rules);
        rule
    }

    pub fn update_rule(&self, rule: RouteRule) -> bool {
        let mut rules = self.list_rules();
        if let Some(slot) = rules.iter_mut().find(|r| r.id == rule.id) {
            *slot = rule;
            self.save_rules(&rules);
            true
        } else {
            false
        }
    }

    pub fn delete_rule(&self, id: &str) -> bool {
        let mut rules = self.list_rules();
        let before = rules.len();
        rules.retain(|r| r.id != id);
        let changed = rules.len() != before;
        if changed {
            self.save_rules(&rules);
        }
        changed
    }

    /// 匹配第一条启用且模型前缀命中的规则
    pub fn match_rule(&self, model: &str) -> Option<RouteRule> {
        self.list_rules().into_iter().find(|r| {
            r.enabled && (r.model_pattern == "*" || model.starts_with(&r.model_pattern))
        })
    }

    // ---------- 账号解析 ----------

    /// 平台的全部可用账号凭据（status=failed 的沉底；企业路由只选租户账号）
    pub fn accounts_for(&self, platform: Platform) -> Vec<Cred> {
        let provider = platform.account_provider();
        let Ok(rows) = self.store.list_accounts() else { return Vec::new() };
        let full = self.store.list_accounts_full().unwrap_or_default();
        let mut creds: Vec<(Cred, bool)> = Vec::new();
        for (p, _uid, _name, _secret, auth) in rows {
            if p != provider {
                continue;
            }
            let Ok(c) = serde_json::from_str::<Cred>(&auth) else { continue };
            if c.token.is_empty() {
                continue;
            }
            if platform == Platform::TraeEnterprise {
                let tenant = Self::is_enterprise_cred(&c);
                if !tenant {
                    continue;
                }
            }
            let failed = full.iter().find(|r| r.provider == p && r.uid == c.uid)
                .map(|r| r.status == "failed").unwrap_or(false);
            creds.push((c, failed));
        }
        creds.sort_by_key(|(_, failed)| *failed);
        creds.into_iter().map(|(c, _)| c).collect()
    }

    /// 显式选号（provider:uid）
    pub fn account_by_ref(&self, r#ref: &str) -> Option<Cred> {
        let (p, u) = r#ref.split_once(':')?;
        let rows = self.store.list_accounts().ok()?;
        rows.iter()
            .find(|(ap, au, _, _, _)| ap == p && au == u)
            .and_then(|(_, _, _, _, auth)| serde_json::from_str::<Cred>(auth).ok())
    }

    /// 凭据是否企业租户账号（决定 Trae 是否走企业网关）
    pub fn is_enterprise_cred(cred: &Cred) -> bool {
        cred.meta.get("enterpriseId").and_then(|e| e.as_str()).map(|s| !s.is_empty()).unwrap_or(false)
            || cred.meta.get("tobOnline").and_then(|e| e.as_bool()).unwrap_or(false)
    }
}
