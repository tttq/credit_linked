pub mod trae_local;
pub mod wb_local;
pub mod qoder_local;
pub mod trae;
pub mod workbuddy;
pub mod minimax;
pub mod minimax_local;
pub mod qoder;
pub mod codearts;
pub mod zcode;
pub mod zcode_local;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::PlatformError;

/// 每账号凭据（secret 字段入库前经 vault seal）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct Cred {
    pub provider: String,
    pub uid: String,
    pub name: String,
    pub token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub meta: Value,
}

/// checkin 状态
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct CheckinState {
    pub checked: bool,
    pub claimable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reward_amount: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_manual: Option<bool>,
    #[serde(skip_serializing_if = "Value::is_null", default)]
    pub raw: Value,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Receipt {
    pub status: String, // success | already | failed | pending | needs-action | no-activity
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Quota {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<f64>,
    #[serde(skip_serializing_if = "Value::is_null", default)]
    pub parts: Value,
    pub sampled_at: String,
}

#[async_trait]
pub trait Adapter: Send + Sync {
    fn provider(&self) -> &'static str;
    async fn verify(&self, c: &Cred) -> Result<Value, PlatformError>;
    async fn state(&self, c: &Cred) -> Result<CheckinState, PlatformError>;
    async fn claim(&self, c: &Cred, st: &CheckinState) -> Result<Receipt, PlatformError>;
    async fn quota(&self, c: &Cred) -> Result<Quota, PlatformError>;
    /// 刷新 token；返回可能更新的凭据字段（refresh_token/expires_at）
    async fn refresh(&self, c: &Cred) -> Result<Cred, PlatformError>;
}

pub fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
    // UTC+8 iso（与原版一致用 +08:00 偏移表达）
    let dt = ms + 8 * 3600 * 1000;
    let secs = dt / 1000;
    let days = secs / 86400;
    let rem = secs % 86400;
    let (y, mo, d) = civil_from_days(days);
    format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}.000+08:00",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

pub fn day_cn() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64 + 8 * 3600;
    let (y, mo, d) = civil_from_days(secs / 86400);
    format!("{y:04}-{mo:02}-{d:02}")
}

/// Howard Hinnant civil_from_days
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
