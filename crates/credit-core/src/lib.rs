//! credit-rs 核心库：多平台（Trae/WorkBuddy/Qoder/码道/ZCode 等）签到与额度聚合。
//! 分层：adapters（平台适配）+ net（受限 HTTP）+ crypto/vault（信封解密与凭据加密）
//! + db（SQLite 存储）→ service/login/schedule（面向 UI 与网关的编排入口）。

/// 统一错误类型（网络/鉴权/限频/业务码/本机读取等）。
pub mod error;
/// 受限平台 HTTP 客户端（https 强制 + host 白名单）与 business 响应解包。
pub mod net;
/// 对称信封加解密：Trae tc 信封（AES-CBC）与 Chromium v10 信封（AES-GCM）。
pub mod crypto;
/// 凭据落盘加密（Windows DPAPI / 非 Windows keyring 降级）。
pub mod vault;
/// 各平台 Adapter 抽象与实现（verify/state/claim/quota/refresh）。
pub mod adapters;
/// 签到时间调度：UTC+8 日界 + 固定种子随机窗口。
pub mod schedule;
/// 平台网页/设备码登录编排（会话仅存内存）。
pub mod login;
/// 服务编排：签到/额度/导入/统计等主入口。
pub mod service;
/// SQLite 存储：accounts/history/settings/gateway_logs。
pub mod db;

/// 便捷 re-export：全 crate 统一使用的错误类型。
pub use error::PlatformError;
/// 跨平台 Electron 应用数据目录探测（本机凭据导入用）。
pub mod platform_dirs;
/// 子进程启动工具（Windows 隐藏控制台窗口）。
pub mod proc_util;
