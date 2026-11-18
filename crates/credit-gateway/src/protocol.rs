//! 上游平台协议常量与请求构造（对齐 dsh-connect-workbuddy / dsh-connect-trae 的 wire 层）。
//!
//! ## WorkBuddy（CodeBuddy 腾讯云 / 国际版）
//! - chat: POST {chat_base}/v2/chat/completions —— OpenAI 线格式（choices/delta SSE）
//! - models: GET {chat_base}/v3/config —— code!=0 信封，data.models + data.agents(cli 桶)
//! - CN chat base: https://copilot.tencent.com；intl: https://www.workbuddy.ai（或 *.codebuddy.ai）
//! - 身份头：Authorization Bearer / X-User-Id / X-Domain / X-Product / X-Enterprise-Id
//! - UA（chat 通道）：CLI/2.63.2 CodeBuddy/2.63.2
//!
//! ## Trae（CN SOLO / 企业 / 国际 SG）
//! - chat: POST {chat}/api/agent/v3/llm_utils_chat —— 自定义 SSE 事件流
//!   （event: output → response/reasoning_content；token_usage；done；error）
//! - models: GET {remote}/models?functions=...（data.list[]，name/config_name 才是 chat 真名）
//! - CN chat: https://trae-api-cn.mchost.guru；企业: https://console.enterprise.trae.cn
//!   （企业路由必须带 session_id；拒绝帧是 HTTP 200 首帧 event:error code=4001）
//! - intl chat: https://coresg-normal.trae.ai；remote 目录: solo.trae.cn / coresg-normal.trae.ai
//! - 身份头：Authorization Cloud-IDE-JWT / X-Ide-Token / x-machine-id / x-device-id /
//!   x-app-version-code（必须纯数字）/ x-app-id / Cloud-IDE-JWT UA

use serde_json::{json, Value};

/// WorkBuddy chat 通道 UA（上游按 CLI agent 身份计费限频，逐字对齐）
pub const WB_CHAT_UA: &str = "CLI/2.63.2 CodeBuddy/2.63.2";
/// WorkBuddy 产品配置通道 UA（/v3/config 逐字对齐官方客户端）
pub const WB_PRODUCT_UA: &str = "WorkBuddy/5.8.3 WorkBuddy/5.8.3 CLI/2.161.0";

pub const WB_CN_CHAT_BASE: &str = "https://copilot.tencent.com";
pub const WB_GLOBAL_BASE: &str = "https://www.workbuddy.ai";
pub const WB_CODEBUDDY_AI_BASE: &str = "https://www.codebuddy.ai";

pub const TRAE_CN_CHAT_BASE: &str = "https://trae-api-cn.mchost.guru";
pub const TRAE_ENTERPRISE_HOST: &str = "https://console.enterprise.trae.cn";
pub const TRAE_AI_CHAT_BASE: &str = "https://coresg-normal.trae.ai";
pub const TRAE_CN_REMOTE_BASE: &str = "https://solo.trae.cn/api/remote/v1";
pub const TRAE_AI_REMOTE_BASE: &str = "https://coresg-normal.trae.ai/api/remote/v1";

/// Trae 固定 x-app-id（两个网关同值，实测验证）
pub const TRAE_APP_ID: &str = "6eefa01c-1036-4c7e-9ca5-d891f63bfcd8";
/// 非数字构建版本时的回落（上游绑定必须纯数字）
pub const TRAE_VERSION_CODE_FALLBACK: &str = "20260716";

/// 远程模型目录 functions（CN 与 AI 桶）
pub const TRAE_DIRECTORY_FUNCTIONS_CN: [&str; 12] = [
    "solo_agent", "solo_agent_remote", "solo_work_remote", "solo_work_lite",
    "chat_v3", "solo_coder", "builder_v3", "chat", "inline_chat",
    "solo_agent_lite", "solo_design_lite", "solo_design_remote",
];
pub const TRAE_DIRECTORY_FUNCTIONS_AI: [&str; 12] = [
    "solo_agent", "solo_agent_remote", "solo_work_remote", "solo_work_lite",
    "chat_v3", "solo_coder", "builder_v3", "chat", "inline_chat",
    "solo_agent_lite", "solo_design_lite", "solo_design_remote",
];

/// 网关支持的"平台"（账号路由里的路由键）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    WorkBuddy,        // WorkBuddy 国内版
    WorkBuddyIntl,    // WorkBuddy 国际版
    Trae,             // Trae 国内 SOLO
    TraeEnterprise,   // Trae 国内企业租户
    TraeIntl,         // Trae 国际 SG
    Zcode,            // ZCode（智谱）— open.bigmodel.cn OpenAI 兼容
    Minimax,          // MiniMax — anthropic 兼容通道（OAuth token 或 API key）
    Qoder,            // Qoder — 无公开 chat 协议（签到/额度保留）
    CodeArts,         // 华为码道 — 无公开 chat 协议（签到/额度保留）
}

impl Platform {
    pub fn parse(s: &str) -> Option<Platform> {
        match s {
            "workbuddy" => Some(Platform::WorkBuddy),
            "workbuddy-intl" => Some(Platform::WorkBuddyIntl),
            "trae" => Some(Platform::Trae),
            "trae-enterprise" => Some(Platform::TraeEnterprise),
            "trae-global" => Some(Platform::TraeIntl),
            "zcode" => Some(Platform::Zcode),
            "minimax" => Some(Platform::Minimax),
            "qoder" => Some(Platform::Qoder),
            "codearts" | "codearts-desktop" => Some(Platform::CodeArts),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Platform::WorkBuddy => "workbuddy",
            Platform::WorkBuddyIntl => "workbuddy-intl",
            Platform::Trae => "trae",
            Platform::TraeEnterprise => "trae-enterprise",
            Platform::TraeIntl => "trae-global",
            Platform::Zcode => "zcode",
            Platform::Minimax => "minimax",
            Platform::Qoder => "qoder",
            Platform::CodeArts => "codearts",
        }
    }

    pub fn all() -> [Platform; 9] {
        [Platform::WorkBuddy, Platform::WorkBuddyIntl, Platform::Trae, Platform::TraeEnterprise, Platform::TraeIntl,
         Platform::Zcode, Platform::Minimax, Platform::Qoder, Platform::CodeArts]
    }

    pub fn display(&self) -> &'static str {
        match self {
            Platform::WorkBuddy => "WorkBuddy 国内版",
            Platform::WorkBuddyIntl => "WorkBuddy 国际版",
            Platform::Trae => "Trae 国内版 (SOLO)",
            Platform::TraeEnterprise => "Trae 企业版",
            Platform::TraeIntl => "Trae 国际版",
            Platform::Zcode => "ZCode (智谱)",
            Platform::Minimax => "MiniMax",
            Platform::Qoder => "Qoder",
            Platform::CodeArts => "华为码道",
        }
    }

    /// credit-core accounts 表里的 provider 名（账号按它存）
    pub fn account_provider(&self) -> &'static str {
        match self {
            Platform::WorkBuddy => "workbuddy",
            Platform::WorkBuddyIntl => "workbuddy-intl",
            Platform::Trae | Platform::TraeEnterprise => "trae",
            Platform::TraeIntl => "trae-intl",
            Platform::Zcode => "zcode",
            Platform::Minimax => "minimax",
            Platform::Qoder => "qoder",
            Platform::CodeArts => "codearts",
        }
    }

    /// 该平台是否有可用 chat 通道（false = 只保留签到/额度，网关路由时明确报错）
    pub fn chat_capable(&self) -> bool {
        matches!(self,
            Platform::WorkBuddy | Platform::WorkBuddyIntl
            | Platform::Trae | Platform::TraeEnterprise | Platform::TraeIntl
            | Platform::Zcode | Platform::Minimax | Platform::Qoder)
    }

}

/// WorkBuddy chat base：domain 判区（*.codebuddy.ai → 该站，否则国际站）
pub fn wb_chat_base(domain: &str) -> &'static str {
    let lowered = domain.trim().to_lowercase();
    if lowered == "codebuddy.ai" || lowered.ends_with(".codebuddy.ai") {
        WB_CODEBUDDY_AI_BASE
    } else {
        WB_GLOBAL_BASE
    }
}

/// WorkBuddy 是否国际区
pub fn wb_is_global(domain: &str) -> bool {
    let lowered = domain.trim().to_lowercase();
    lowered.contains("workbuddy.ai") || lowered.contains("codebuddy.ai") || lowered.contains("workbuddy.cc")
}

/// WorkBuddy chat 请求头（对齐 chatHeaders；设备指纹头由凭据 meta 提供可选项）
pub fn wb_chat_headers(token: &str, uid: &str, domain: &str, enterprise_id: Option<&str>) -> Vec<(String, String)> {
    let mut hs = vec![
        ("Authorization".to_string(), format!("Bearer {token}")),
        ("Content-Type".to_string(), "application/json".to_string()),
        ("Accept".to_string(), "text/event-stream".to_string()),
        ("User-Agent".to_string(), WB_CHAT_UA.to_string()),
        ("X-Product".to_string(), "WorkBuddy".to_string()),
        ("X-IDE-Name".to_string(), "WorkBuddy".to_string()),
        ("X-IDE-Type".to_string(), "WorkBuddy".to_string()),
        ("X-Agent-Purpose".to_string(), "conversation".to_string()),
        ("Accept-Language".to_string(), if wb_is_global(domain) { "en-US".to_string() } else { "zh-CN".to_string() }),
    ];
    if uid.is_empty() {
        hs.push(("X-No-User-Id".to_string(), "1".to_string()));
    } else {
        hs.push(("X-User-Id".to_string(), uid.to_string()));
    }
    if enterprise_id.unwrap_or_default().is_empty() {
        hs.push(("X-No-Enterprise-Id".to_string(), "1".to_string()));
    } else {
        hs.push(("X-Enterprise-Id".to_string(), enterprise_id.unwrap().to_string()));
        hs.push(("X-Tenant-Id".to_string(), enterprise_id.unwrap().to_string()));
    }
    if domain.is_empty() {
        hs.push(("X-No-Department-Info".to_string(), "1".to_string()));
    } else {
        hs.push(("X-Domain".to_string(), domain.to_string()));
    }
    hs
}

/// WorkBuddy 模型目录请求头（产品配置通道）
pub fn wb_config_headers(token: &str, uid: &str, domain: &str, global: bool) -> Vec<(String, String)> {
    let mut hs = vec![
        ("Authorization".to_string(), format!("Bearer {token}")),
        ("Accept".to_string(), "application/json".to_string()),
        ("User-Agent".to_string(), WB_PRODUCT_UA.to_string()),
    ];
    if global {
        hs.push(("X-Product".to_string(), "SaaS".to_string()));
        hs.push(("X-Requested-With".to_string(), "XMLHttpRequest".to_string()));
        hs.push(("Connection".to_string(), "close".to_string()));
    } else {
        let origin = WB_CN_CHAT_BASE;
        hs.push(("Origin".to_string(), origin.to_string()));
        hs.push(("Referer".to_string(), format!("{origin}/")));
    }
    if !uid.is_empty() {
        hs.push(("X-User-Id".to_string(), uid.to_string()));
    }
    if !domain.is_empty() {
        hs.push(("X-Domain".to_string(), domain.to_string()));
    }
    hs
}

/// 规范化 Trae 构建版本号：非纯数字 → 回落
pub fn trae_version_code(build_version: Option<&str>) -> String {
    match build_version {
        Some(v) if !v.trim().is_empty() && v.trim().chars().all(|c| c.is_ascii_digit()) => v.trim().to_string(),
        _ => TRAE_VERSION_CODE_FALLBACK.to_string(),
    }
}

/// Trae chat 请求头（对齐 buildTraeHeaders 的 agent-task profile）
/// identity: machine_id / device_id / build_version / app_version 均来自凭据 meta（本机扫描或登录时入库）
pub fn trae_chat_headers(token: &str, meta: &Value) -> Vec<(String, String)> {
    let request_id = uuid::Uuid::new_v4().to_string();
    let trace_id: String = request_id.replace('-', "").chars().take(32).collect();
    let machine_id = meta.get("machineId").and_then(|v| v.as_str()).unwrap_or("");
    let device_id = meta.get("deviceId").and_then(|v| v.as_str()).unwrap_or("");
    let build = meta.get("buildVersion").and_then(|v| v.as_str()).map(|s| s.to_string());
    let app_version = meta.get("appVersion").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let version_code = trae_version_code(build.as_deref());
    let os_version = meta.get("osVersion").and_then(|v| v.as_str()).unwrap_or("Windows");
    let device_brand = meta.get("deviceBrand").and_then(|v| v.as_str()).unwrap_or("PC");

    vec![
        ("Authorization".to_string(), format!("Cloud-IDE-JWT {token}")),
        ("X-Ide-Token".to_string(), token.to_string()),
        ("X-Cloudide-Token".to_string(), token.to_string()),
        ("x-plugin-channel".to_string(), "icube-ai".to_string()),
        ("User-Agent".to_string(), if app_version.is_empty() { format!("Trae/{version_code}") } else { format!("Trae/{app_version}") }),
        ("x-app-id".to_string(), TRAE_APP_ID.to_string()),
        ("x-machine-id".to_string(), machine_id.to_string()),
        ("x-device-id".to_string(), device_id.to_string()),
        ("x-device-type".to_string(), "windows".to_string()),
        ("x-device-brand".to_string(), device_brand.to_string()),
        ("x-os-version".to_string(), os_version.to_string()),
        ("x-app-version-code".to_string(), version_code.clone()),
        ("x-ide-version-code".to_string(), version_code),
        ("x-custom-trace-id".to_string(), trace_id.clone()),
        ("x-flow-traceparent".to_string(), format!("04-{trace_id}-{}-01", &trace_id[..16.min(trace_id.len())])),
        ("request-traffic-type".to_string(), "prod".to_string()),
        ("Content-Type".to_string(), "application/json".to_string()),
        ("Accept".to_string(), "text/event-stream".to_string()),
    ]
}

/// Trae 模型目录请求头（remote web 通道）
pub fn trae_remote_headers(token: &str, intl: bool) -> Vec<(String, String)> {
    let (referer, tz, lang) = if intl {
        ("https://coresg-normal.trae.ai/", "Asia/Singapore", "en")
    } else {
        ("https://solo.trae.cn/", "Asia/Shanghai", "zh-cn")
    };
    vec![
        ("Authorization".to_string(), format!("Cloud-IDE-JWT {token}")),
        ("Content-Type".to_string(), "application/json".to_string()),
        ("x-trae-client-type".to_string(), "web".to_string()),
        ("x-trae-user-timezone".to_string(), tz.to_string()),
        ("x-preferenced-language".to_string(), lang.to_string()),
        ("Referer".to_string(), referer.to_string()),
        ("User-Agent".to_string(), "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36".to_string()),
    ]
}

/// Trae chat body（对齐 prepareSoloBody 的 SOLO 形状 + 企业路由补 session_id）
pub fn trae_chat_body(messages: &[Value], model: &str, max_tokens: Option<u32>, reasoning_effort: Option<&str>, enterprise: bool) -> Value {
    let request_id = uuid::Uuid::new_v4().to_string();
    let mut body = json!({
        "messages": messages,
        "model": model,
        "config_name": model,
        "function": "solo_work_lite",
        "stream": true,
        "request_id": request_id,
        "session_id": request_id,
    });
    if enterprise {
        // 企业网关强制每次调用独立 session_id
        body["session_id"] = json!(uuid::Uuid::new_v4().to_string());
    }
    if let Some(mt) = max_tokens {
        body["max_tokens"] = json!(mt);
    }
    if let Some(effort) = reasoning_effort {
        body["reasoning_effort"] = json!(effort);
    }
    body
}

/// OpenAI messages → 上游 messages（string content 展平为 [{type:"text"}]；developer→system）
pub fn normalize_messages(openai_messages: &[Value]) -> (Vec<Value>, String) {
    let mut out: Vec<Value> = Vec::new();
    let mut system_parts: Vec<String> = Vec::new();
    for m in openai_messages {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
        let content = m.get("content").cloned().unwrap_or(Value::Null);
        let text = match &content {
            Value::String(s) => s.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|p| {
                    if p.get("type").and_then(|t| t.as_str()) == Some("text") {
                        p.get("text").and_then(|t| t.as_str()).map(|s| s.to_string())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        match role {
            "system" | "developer" => system_parts.push(text),
            "user" | "assistant" | "tool" => {
                let role = if role == "tool" { "user" } else { role };
                out.push(json!({
                    "role": role,
                    "content": [{ "type": "text", "text": text }],
                }));
            }
            _ => {}
        }
    }
    (out, system_parts.join("\n"))
}

/// WorkBuddy chat body（对齐 prepareChatBody：强制 stream、developer→system、保证 system 领头）
pub fn wb_chat_body(messages: &[Value], system: &str, model: &str, max_tokens: Option<u32>, reasoning_effort: Option<&str>, uid: &str) -> Value {
    let mut msgs: Vec<Value> = Vec::new();
    if !system.is_empty() {
        msgs.push(json!({ "role": "system", "content": system }));
    }
    for m in messages {
        // 上游要求 content 为数组形状；消息白名单字段（role/content），丢弃外部客户端附加字段（name/tool_calls 等指纹噪声）
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user").to_string();
        let content = m.get("content").cloned().unwrap_or(json!([]));
        let mut mm = json!({ "role": role, "content": content });
        if mm.get("content").map(|c| c.is_string()).unwrap_or(false) {
            let text = mm["content"].as_str().unwrap_or("").to_string();
            mm["content"] = json!([{ "type": "text", "text": text }]);
        }
        msgs.push(mm);
    }
    let mut body = json!({
        "messages": msgs,
        "model": model,
        "stream": true,
        "user": if uid.is_empty() { "credit-rs" } else { uid },
    });
    if let Some(mt) = max_tokens {
        body["max_tokens"] = json!(mt);
    }
    if let Some(effort) = reasoning_effort {
        body["reasoning_effort"] = json!(effort);
    }
    body
}

// ============================ ZCode（智谱） ============================

/// 智谱开放平台 OpenAI 兼容端点
pub const ZCODE_CHAT_BASE: &str = "https://open.bigmodel.cn";
/// 智谱 chat 路径（OpenAI 兼容）
pub const ZCODE_CHAT_PATH: &str = "/api/paas/v4/chat/completions";

/// ZCode chat 请求头：凭据即智谱 access_token；deviceMid 可选
pub fn zcode_chat_headers(token: &str, device_mid: Option<&str>) -> Vec<(String, String)> {
    let mut hs = vec![
        ("Authorization".to_string(), format!("Bearer {token}")),
        ("Content-Type".to_string(), "application/json".to_string()),
        ("Accept".to_string(), "application/json".to_string()),
    ];
    if let Some(mid) = device_mid {
        if !mid.is_empty() {
            hs.push(("X-Device-Id".to_string(), mid.to_string()));
        }
    }
    hs
}

/// 智谱 chat body（OpenAI 兼容；thinking 可选参数）
pub fn zcode_chat_body(messages: &[Value], system: &str, model: &str, max_tokens: Option<u32>, thinking: bool) -> Value {
    let mut msgs: Vec<Value> = Vec::new();
    if !system.is_empty() {
        msgs.push(json!({ "role": "system", "content": system }));
    }
    msgs.extend(messages.iter().cloned());
    let mut body = json!({
        "model": model,
        "messages": msgs,
        "stream": true,
    });
    if let Some(mt) = max_tokens {
        body["max_tokens"] = json!(mt);
    }
    // 智谱 GLM-4.5+ 的思考开关
    body["thinking"] = json!({ "type": if thinking { "enabled" } else { "disabled" } });
    body
}

// ============================ MiniMax ============================

/// MiniMax 官方 API（Anthropic 兼容 + OpenAI 兼容双通道）
pub const MINIMAX_API_BASE: &str = "https://agent.minimax.cn";
/// MiniMax anthropic 兼容 chat 路径
pub const MINIMAX_ANTHROPIC_PATH: &str = "/mavis/api/v1/llm/v1/messages";

/// MiniMax chat 请求头（凭据：MMOAT OAuth token 或 API key）
pub fn minimax_chat_headers(token: &str) -> Vec<(String, String)> {
    vec![
        ("Authorization".to_string(), format!("Bearer {token}")),
        ("x-api-key".to_string(), token.to_string()),
        ("anthropic-version".to_string(), "2023-06-01".to_string()),
        ("Content-Type".to_string(), "application/json".to_string()),
        ("Accept".to_string(), "application/json".to_string()),
    ]
}

/// MiniMax Anthropic Messages body（Anthropic 格式与 OpenAI 不同：system 是顶层字段）
pub fn minimax_chat_body(messages: &[Value], system: &str, model: &str, max_tokens: Option<u32>) -> Value {
    // Anthropic messages 不支持 system role → 移到顶层 system
    let msgs: Vec<Value> = messages.iter()
        .filter(|m| m.get("role").and_then(|r| r.as_str()) != Some("system"))
        .cloned()
        .collect();
    let mut body = json!({
        "model": model,
        "messages": msgs,
        "max_tokens": max_tokens.unwrap_or(4096),
        "stream": true,
    });
    if !system.is_empty() {
        body["system"] = json!(system);
    }
    body
}

/// 解析 Anthropic SSE chunk → 事件列表（MiniMax Anthropic 兼容通道返回 Anthropic 线格式）
pub fn parse_anthropic_chunk(v: &Value) -> Vec<crate::upstream::StreamEvent> {
    use crate::upstream::StreamEvent;
    let mut out = Vec::new();
    let typ = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match typ {
        "error" => {
            let err = v.get("error").cloned().unwrap_or(Value::Null);
            let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("upstream error").to_string();
            out.push(StreamEvent::Error(msg, "anthropic_error".into()));
        }
        "content_block_delta" => {
            let delta = v.get("delta").cloned().unwrap_or(Value::Null);
            match delta.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "text_delta" => {
                    if let Some(t) = delta.get("text").and_then(|t| t.as_str()) {
                        if !t.is_empty() {
                            out.push(StreamEvent::Text(t.to_string()));
                        }
                    }
                }
                "thinking_delta" => {
                    if let Some(t) = delta.get("thinking").and_then(|t| t.as_str()) {
                        if !t.is_empty() {
                            out.push(StreamEvent::Reasoning(t.to_string()));
                        }
                    }
                }
                _ => {}
            }
        }
        "message_delta" => {
            // 携带 stop_reason 与 usage
            if let Some(u) = v.get("usage") {
                let input = u.get("input_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
                let output = u.get("output_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
                out.push(StreamEvent::Usage { input, output, total: Some(input + output) });
            }
            if let Some(sr) = v.get("delta").and_then(|d| d.get("stop_reason")).and_then(|s| s.as_str()) {
                out.push(StreamEvent::Done { finish: sr.to_string() });
            }
        }
        "message_start" => {
            if let Some(u) = v.pointer("/message/usage") {
                let input = u.get("input_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
                let output = u.get("output_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
                if input > 0 || output > 0 {
                    out.push(StreamEvent::Usage { input, output, total: Some(input + output) });
                }
            }
        }
        _ => {}
    }
    out
}

/// Anthropic SSE 缓冲类型（data: 行，与 OpenAI 相同帧格式）
pub fn anthropic_frame_decode(data: &str) -> Vec<crate::upstream::StreamEvent> {
    if data == "[DONE]" {
        return vec![crate::upstream::StreamEvent::Done { finish: "stop".into() }];
    }
    let Ok(v) = serde_json::from_str::<Value>(data) else { return Vec::new() };
    parse_anthropic_chunk(&v)
}

/// Qoder 云端 chat body（agent_chat_generation 信封格式：model/messages/stream/session_id/request_id）
pub fn qoder_chat_body(messages: &[Value], model: &str) -> Value {
    let request_id = uuid::Uuid::new_v4().to_string();
    // messages 转为 OpenAI 线格式（string content 展平）
    let mut msgs: Vec<Value> = Vec::new();
    for m in messages {
        let mut mm = m.clone();
        if mm.get("content").map(|c| c.is_string()).unwrap_or(false) {
            let text = mm["content"].as_str().unwrap_or("").to_string();
            mm["content"] = json!([{ "type": "text", "text": text }]);
        }
        msgs.push(mm);
    }
    json!({
        "model": model,
        "messages": msgs,
        "stream": true,
        "session_id": request_id,
        "request_id": request_id,
    })
}

