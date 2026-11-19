//! 平台统一错误类型：按失败类别区分，Display 输出面向 UI 展示与落库。

use std::fmt;

/// 平台交互统一错误：调用方按变体决定重试（Network/Rate）、刷新凭据（Auth）或提示用户。
#[derive(Debug, Clone)]
pub enum PlatformError {
    /// 网络层失败（DNS/TLS/超时/非白名单 host/非 https）
    Network(String),
    /// HTTP 401
    Auth,
    /// HTTP 429
    Rate,
    /// HTTP 403
    Forbidden,
    /// HTTP OK 但响应不是合法 JSON / 结构无法识别
    Format(String),
    /// 业务 code 不在 {0,"0",200,"200"}
    Business { code: serde_json::Value, message: String },
    /// 本机凭据与在线身份不一致 / TOCTOU 检测到变化
    Identity(String),
    /// 领取前置条件未满足（如国际版 40901）
    Conditions,
    /// 风控拒绝（Trae 9074 等）
    Restricted(String),
    /// 豆包必须走本机连接
    DoubaoConnect(String),
    /// 本机读取失败
    Local(String),
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlatformError::Network(m) => write!(f, "NETWORK: {}", m),
            PlatformError::Auth => write!(f, "AUTH: 401 未授权"),
            PlatformError::Rate => write!(f, "RATE: 429 限频"),
            PlatformError::Forbidden => write!(f, "FORBIDDEN: 403"),
            PlatformError::Format(m) => write!(f, "FORMAT: {}", m),
            PlatformError::Business { code, message } => write!(f, "BUSINESS code={} {}", code, message),
            PlatformError::Identity(m) => write!(f, "IDENTITY: {}", m),
            PlatformError::Conditions => write!(f, "CONDITIONS: 需先满足使用条件"),
            PlatformError::Restricted(m) => write!(f, "RESTRICTED: {}", m),
            PlatformError::DoubaoConnect(m) => write!(f, "DOUBAO_CONNECT: {}", m),
            PlatformError::Local(m) => write!(f, "LOCAL: {}", m),
        }
    }
}

impl std::error::Error for PlatformError {}

impl PlatformError {
    /// 从响应体提取 (code, msg) 构造 BUSINESS 错误（保留原码原文用于展示/透传）。
    pub fn business_of(v: &serde_json::Value) -> Self {
        PlatformError::Business {
            code: v.get("code").cloned().unwrap_or(serde_json::Value::Null),
            message: v.get("msg").and_then(|m| m.as_str()).unwrap_or("").to_string(),
        }
    }
}
