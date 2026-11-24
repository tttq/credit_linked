//! 平台适配层：Adapter 统一抽象（verify/state/claim/quota/refresh）+ 各平台实现与公共时间工具。
//! *_local 为本机凭据扫描/读取（供「导入」），其余为在线 API 实现。

/// Trae 本机凭据扫描（storage.json tc 信封解密、设备身份、保活）。
pub mod trae_local;
/// WorkBuddy（CodeBuddy）本机会话读取（含新版字段级加密信封）。
pub mod wb_local;
/// Qoder 本机 auth.v1.dat 读取（AES 解密）。
pub mod qoder_local;
/// Trae 在线 API 适配（登录换票、回调解析、签到、设备身份）。
pub mod trae;
/// WorkBuddy 在线 API 适配（国内/国际双线）。
pub mod workbuddy;
/// MiniMax 在线 API 适配（device flow 登录、签到）。
pub mod minimax;
/// MiniMax Code 本机客户端凭据读取。
pub mod minimax_local;
/// Qoder 在线 API 适配（国内/国际双线）。
pub mod qoder;
/// 华为云码道（CodeArts）在线 API 适配（STS 换票、DPoP 签名）。
pub mod codearts;
/// 码道本机会话读取（必要时可拉起客户端）。
pub mod codearts_local;
/// ZCode（智谱）在线 API 适配。
pub mod zcode;
/// ZCode 本机客户端凭据读取。
pub mod zcode_local;
/// ZCode 客户端模拟器（完整模拟官方客户端行为）。
pub mod zcode_client_simulator;

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

/// 签到动作回执：status ∈ success|already|failed|pending|needs-action|no-activity。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Receipt {
    pub status: String, // success | already | failed | pending | needs-action | no-activity
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// 额度快照：remaining 主余额，parts 平台原始明细，sampled_at 采样时间。
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
    /// 平台标识（与 accounts.provider、凭据 provider 一致）。
    fn provider(&self) -> &'static str;
    /// 校验凭据有效性并返回身份信息；401 表示凭据已失效。
    async fn verify(&self, c: &Cred) -> Result<Value, PlatformError>;
    /// 查询今日签到状态（是否已签 / 是否可领）。
    async fn state(&self, c: &Cred) -> Result<CheckinState, PlatformError>;
    /// 执行签到领取并返回回执。
    async fn claim(&self, c: &Cred, st: &CheckinState) -> Result<Receipt, PlatformError>;
    /// 查询账号额度快照。
    async fn quota(&self, c: &Cred) -> Result<Quota, PlatformError>;
    /// 刷新 token；返回可能更新的凭据字段（refresh_token/expires_at）
    async fn refresh(&self, c: &Cred) -> Result<Cred, PlatformError>;
}

/// 当前时间 ISO 字符串（UTC+8 偏移表达；落库与展示统一格式）。
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

/// 当前 UTC+8 日期 "YYYY-MM-DD"（签到 cycle 标识，一天一签的判重键）。
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

/// 天数（自 1970-01-01 起）→ (年, 月, 日)：Howard Hinnant civil_from_days 逆运算。
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
