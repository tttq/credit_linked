//! credit-gateway：账号路由网关（像本地 Ollama 一样对外提供 OpenAI 标准接口）。
//!
//! - 平台协议层（protocol.rs）：WorkBuddy / Trae 双平台上游请求构造
//! - 上游调用层（upstream.rs）：模型目录抓取 + chat SSE 流转发
//! - 账号路由（router.rs）：当前平台切换 + 路由规则 + 多账号 failover
//! - HTTP 服务（server.rs）：/v1/models、/v1/chat/completions、/admin/*

pub mod protocol;
pub mod router;
pub mod server;
pub mod upstream;

pub use router::{RouteRule, Router};
pub use server::{device_salt_init, serve, GatewayState};
pub use upstream::ModelInfo;
