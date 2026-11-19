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

/// WorkBuddy chat 通道 UA（官方 VlmUpstream 格式：WorkBuddy/{ver} WorkBuddy/{ver} [CLI/{cliVer}]）
pub const WB_CHAT_UA: &str = "WorkBuddy/5.11.3 WorkBuddy/5.11.3 CLI/2.63.2";
/// WorkBuddy 产品配置通道 UA（/v3/config 逐字对齐官方客户端）
pub const WB_PRODUCT_UA: &str = "WorkBuddy/5.8.3 WorkBuddy/5.8.3 CLI/2.161.0";

/// WorkBuddy 国内 chat 网关
pub const WB_CN_CHAT_BASE: &str = "https://copilot.tencent.com";
/// WorkBuddy 国际 chat 网关
pub const WB_GLOBAL_BASE: &str = "https://www.workbuddy.ai";
/// WorkBuddy 国际备用域（*.codebuddy.ai 域名走此站）
pub const WB_CODEBUDDY_AI_BASE: &str = "https://www.codebuddy.ai";

/// Trae 国内 SOLO chat 网关
pub const TRAE_CN_CHAT_BASE: &str = "https://trae-api-cn.mchost.guru";
/// Trae 企业租户 chat 网关（路由必须带 session_id）
pub const TRAE_ENTERPRISE_HOST: &str = "https://console.enterprise.trae.cn";
/// Trae 国际（新加坡）chat 网关
pub const TRAE_AI_CHAT_BASE: &str = "https://coresg-normal.trae.ai";
/// Trae 国内模型目录（remote web 通道）
pub const TRAE_CN_REMOTE_BASE: &str = "https://solo.trae.cn/api/remote/v1";
/// Trae 国际模型目录（remote web 通道）
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
/// 远程模型目录 functions（AI/国际桶；与 CN 桶当前同集）
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
    /// 平台字符串 → Platform（账号路由与请求 "平台/模型" 前缀解析用）
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

    /// Platform → 规范字符串（settings 存储与 API 展示用）
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

    /// 全部平台（管理面清单顺序）
    pub fn all() -> [Platform; 9] {
        [Platform::WorkBuddy, Platform::WorkBuddyIntl, Platform::Trae, Platform::TraeEnterprise, Platform::TraeIntl,
         Platform::Zcode, Platform::Minimax, Platform::Qoder, Platform::CodeArts]
    }

    /// 中文显示名（UI 用）
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

    /// 该平台是否有可用 chat 通道
    pub fn chat_capable(&self) -> bool {
        matches!(self,
            Platform::WorkBuddy | Platform::WorkBuddyIntl
            | Platform::Trae | Platform::TraeEnterprise | Platform::TraeIntl
            | Platform::Zcode | Platform::Minimax | Platform::Qoder | Platform::CodeArts)
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
pub fn wb_chat_headers(token: &str, uid: &str, domain: &str, enterprise_id: Option<&str>, device: Option<(&str, &str, &str)>) -> Vec<(String, String)> {
    // 逐字对齐官方客户端 VlmUpstream/applyGatewayHeaders（asar 逆向）
    let mut hs = vec![
        ("Authorization".to_string(), format!("Bearer {token}")),
        ("Content-Type".to_string(), "application/json".to_string()),
        ("Accept".to_string(), "application/json".to_string()),
        ("x-requested-with".to_string(), "XMLHttpRequest".to_string()),
        ("User-Agent".to_string(), WB_CHAT_UA.to_string()),
        ("X-Product".to_string(), "SaaS".to_string()),
        ("X-IDE-Type".to_string(), "WorkBuddy".to_string()),
        ("X-IDE-Name".to_string(), "WorkBuddy".to_string()),
        ("X-IDE-Version".to_string(), "5.11.3".to_string()),
        ("X-Private-Data".to_string(), "true".to_string()),
        ("X-Agent-Intent".to_string(), "craft".to_string()),
        ("X-Agent-Purpose".to_string(), "conversation".to_string()),
        ("x-codebuddy-request".to_string(), "1".to_string()),
        ("Accept-Encoding".to_string(), "identity".to_string()),
        ("Accept-Language".to_string(), if wb_is_global(domain) { "en-US".to_string() } else { "zh-CN".to_string() }),
        // 会话标识（官方每会话固定 / 每消息随机）
        ("X-Conversation-ID".to_string(), uuid::Uuid::new_v4().to_string()),
        ("X-Conversation-Request-ID".to_string(), uuid::Uuid::new_v4().to_string()),
        ("acp-connection-id".to_string(), uuid::Uuid::new_v4().to_string()),
        ("X-Conversation-Message-ID".to_string(), uuid::Uuid::new_v4().to_string()),
        ("X-Request-ID".to_string(), uuid::Uuid::new_v4().to_string()),
    ];
    // trace 链路头（官方 applyTraceHeaders：W3C traceparent + b3 多头）
    let trace_id: String = uuid::Uuid::new_v4().simple().to_string();
    let span_id: String = uuid::Uuid::new_v4().simple().to_string()[..16].to_string();
    let parent_id: String = uuid::Uuid::new_v4().simple().to_string()[..16].to_string();
    hs.push(("traceparent".to_string(), format!("00-{trace_id}-{span_id}-01")));
    hs.push(("b3".to_string(), format!("{trace_id}-{span_id}-1-{parent_id}")));
    hs.push(("X-B3-TraceId".to_string(), trace_id.clone()));
    hs.push(("X-B3-ParentSpanId".to_string(), parent_id.clone()));
    hs.push(("X-B3-SpanId".to_string(), span_id));
    hs.push(("X-B3-Sampled".to_string(), "1".to_string()));
    hs.push(("X-Trace-ID".to_string(), trace_id));
    // 设备身份（Turing 真实 token + 盐派生 machine/session，对齐官方客户端）
    if let Some((dev_token, machine, session)) = device {
        if !dev_token.is_empty() {
            hs.push(("X-Device-Token".to_string(), dev_token.to_string()));
        }
        if !machine.is_empty() {
            hs.push(("X-Machine-ID".to_string(), machine.to_string()));
        }
        if !session.is_empty() {
            hs.push(("X-Session-ID".to_string(), session.to_string()));
        }
    }
    // 用户/企业/域
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

/// Trae 对话请求头（对齐 dsh-connect-trae buildTraeHeaders，SOLO 通道需要 x-uid/请求 id）
/// identity: machine_id / device_id / build_version / app_version 均来自凭据 meta（本机扫描或登录时入库）
pub fn trae_chat_headers(token: &str, meta: &Value) -> Vec<(String, String)> {
    let uid = meta.get("uid").and_then(|v| v.as_str()).unwrap_or("");
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
        ("x-app-version".to_string(), app_version.clone()),
        ("x-ide-version".to_string(), app_version.clone()),
        ("x-ide-version-type".to_string(), "stable".to_string()),
        ("x-custom-trace-id".to_string(), trace_id.clone()),
        ("x-flow-traceparent".to_string(), format!("04-{trace_id}-{}-01", &trace_id[..16.min(trace_id.len())])),
        ("request-traffic-type".to_string(), "prod".to_string()),
        ("x-uid".to_string(), uid.to_string()),
        ("x-request-id".to_string(), request_id.clone()),
        ("x-trae-request-id".to_string(), request_id),
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

/// Trae chat body（对齐 prepareSoloBody 的 SOLO 形状 + 企业路由补 session_id）。
/// `function` 必须与模型所属通道一致：模型只在列出它的 function 下可调用
/// （如 glm-5.3 在 CN 上仅 solo_work_remote 提供），映射来自模型目录。
pub fn trae_chat_body(messages: &[Value], model: &str, function: &str, max_tokens: Option<u32>, reasoning_effort: Option<&str>, enterprise: bool) -> Value {
    let request_id = uuid::Uuid::new_v4().to_string();
    let mut body = json!({
        "messages": messages,
        "model": model,
        "config_name": model,
        "function": if function.is_empty() { "solo_work_lite" } else { function },
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
/// 保留 Agent 多轮工具调用所需的 assistant.tool_calls / tool.tool_call_id。
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
            "tool" => {
                // OpenAI 线格式：tool 消息 content 必须是字符串，且需带 tool_call_id
                let mut msg = json!({ "role": "tool", "content": text });
                if let Some(id) = m.get("tool_call_id") {
                    msg["tool_call_id"] = id.clone();
                }
                out.push(msg);
            }
            "user" | "assistant" => {
                let mut msg = json!({
                    "role": role,
                    "content": [{ "type": "text", "text": text }],
                });
                if role == "assistant" {
                    if let Some(tc) = m.get("tool_calls") {
                        msg["tool_calls"] = tc.clone();
                    }
                }
                out.push(msg);
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
        // 上游要求 content 为数组形状；保留 Agent 工具调用回合的 tool_calls / tool_call_id
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user").to_string();
        let content = m.get("content").cloned().unwrap_or(json!([]));
        let mut mm = json!({ "role": role, "content": content });
        // tool 消息的 content 保持字符串（OpenAI 线格式要求）
        if role != "tool" && mm.get("content").map(|c| c.is_string()).unwrap_or(false) {
            let text = mm["content"].as_str().unwrap_or("").to_string();
            mm["content"] = json!([{ "type": "text", "text": text }]);
        }
        if role == "assistant" {
            if let Some(tc) = m.get("tool_calls") { mm["tool_calls"] = tc.clone(); }
        }
        if role == "tool" {
            if let Some(id) = m.get("tool_call_id") { mm["tool_call_id"] = id.clone(); }
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

// ============================ OpenAI ↔ Anthropic 消息转换 ============================

/// OpenAI content（字符串 / 块数组）→ 纯文本（非文本块忽略）
fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
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
    }
}

/// OpenAI data/url 图片 → Anthropic image block（data URL → base64 source；http(s) → url source）
fn anthropic_image_block(url: &str) -> Option<Value> {
    if let Some(rest) = url.strip_prefix("data:") {
        let (meta, data) = rest.split_once(',')?;
        if data.is_empty() {
            return None;
        }
        let media = meta.strip_suffix(";base64").unwrap_or(meta);
        Some(json!({
            "type": "image",
            "source": { "type": "base64", "media_type": media, "data": data },
        }))
    } else if url.starts_with("http://") || url.starts_with("https://") {
        Some(json!({
            "type": "image",
            "source": { "type": "url", "url": url },
        }))
    } else {
        None
    }
}

/// OpenAI content（字符串 / 块数组）→ Anthropic content blocks（文本 + 可选图片）
fn content_blocks_of(content: Option<&Value>, allow_images: bool) -> Vec<Value> {
    match content {
        Some(Value::String(s)) if !s.is_empty() => vec![json!({ "type": "text", "text": s })],
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| match p.get("type").and_then(|t| t.as_str()) {
                Some("text") => p
                    .get("text")
                    .and_then(|t| t.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| json!({ "type": "text", "text": s })),
                Some("image_url") if allow_images => {
                    let url = p
                        .pointer("/image_url/url")
                        .or_else(|| p.get("url"))
                        .and_then(|u| u.as_str())?;
                    anthropic_image_block(url)
                }
                Some("image") if allow_images => p
                    .get("source")
                    .cloned()
                    .map(|source| json!({ "type": "image", "source": source })),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Anthropic tool_use.id 约束：`^[a-zA-Z0-9_-]{1,64}$`；不合法/缺失 → None（由调用方兜底）
fn anthropic_tool_id(raw: Option<&str>) -> Option<String> {
    let s = raw?.trim();
    if s.is_empty() {
        return None;
    }
    let cleaned: String = s
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if cleaned.is_empty() || cleaned.len() > 64 {
        None
    } else {
        Some(cleaned)
    }
}

/// 追加消息块：相邻同 role 消息合并（Anthropic 要求 user/assistant 交替出现）。
/// 例外：含 tool_result 的 user 消息只与「同样是 tool_result 组」的消息合并，
/// 普通用户文本不会被混进工具结果组，避免 tool_result 不再是消息首块。
fn push_anthropic_blocks(out: &mut Vec<Value>, role: &str, blocks: Vec<Value>) {
    if blocks.is_empty() {
        return;
    }
    let first_is_tool_result = blocks
        .first()
        .and_then(|b| b.get("type"))
        .and_then(|t| t.as_str())
        == Some("tool_result");
    if let Some(last) = out.last_mut() {
        if last.get("role").and_then(|r| r.as_str()) == Some(role) {
            let last_is_tool_group = last
                .get("content")
                .and_then(|c| c.as_array())
                .and_then(|a| a.first())
                .and_then(|b| b.get("type"))
                .and_then(|t| t.as_str())
                == Some("tool_result");
            let mergeable = if first_is_tool_result || last_is_tool_group {
                first_is_tool_result && last_is_tool_group
            } else {
                true
            };
            if mergeable {
                if let Some(arr) = last.get_mut("content").and_then(|c| c.as_array_mut()) {
                    arr.extend(blocks);
                    return;
                }
            }
        }
    }
    out.push(json!({ "role": role, "content": blocks }));
}

/// 追加 tool_result 块：必须位于 user 消息内容首位（Anthropic 约束）。
/// 已存在 tool_result 组则按序插到组末尾；否则插到上一条 user 消息最前面；再否则新建 user 消息。
fn push_tool_results(out: &mut Vec<Value>, blocks: Vec<Value>) {
    if blocks.is_empty() {
        return;
    }
    if let Some(last) = out.last_mut() {
        if last.get("role").and_then(|r| r.as_str()) == Some("user") {
            if let Some(arr) = last.get_mut("content").and_then(|c| c.as_array_mut()) {
                let pos = arr
                    .iter()
                    .take_while(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
                    .count();
                let tail = arr.split_off(pos);
                arr.extend(blocks);
                arr.extend(tail);
                return;
            }
        }
    }
    out.push(json!({ "role": "user", "content": blocks }));
}

/// 为「有 tool_use 但没有对应 tool 结果」的 assistant 回合补 tool_result。
/// 典型场景：长上下文截断 / 客户端中断 / 历史不完整——官方客户端同样补
/// `{"content":"No result provided","is_error":true}`，否则上游 400。
fn synth_missing_tool_results(out: &mut Vec<Value>, pending: &mut std::collections::VecDeque<String>) {
    if pending.is_empty() {
        return;
    }
    let blocks: Vec<Value> = pending
        .drain(..)
        .map(|id| {
            json!({
                "type": "tool_result",
                "tool_use_id": id,
                "content": "No result provided (request interrupted or context truncated)",
                "is_error": true,
            })
        })
        .collect();
    push_tool_results(out, blocks);
}

/// OpenAI 线格式 messages → Anthropic 线格式 messages（system 由调用方放顶层）。
///
/// 关键转换（此前缺失导致 MiniMax 400 `tool result's tool id() not found (2013)`）：
/// - assistant.tool_calls[] → assistant content 里的 `{type:"tool_use", id, name, input}`
/// - role=tool + tool_call_id → user content 里的 `{type:"tool_result", tool_use_id, content}`
/// - 连续 tool_result 合并进同一条 user 消息，且必须是该 user 消息的首块（Anthropic 规范）
/// - 孤儿 tool_result（id 不在紧邻的 assistant tool_use 里）丢弃，避免 upstream 报 unexpected id
/// - assistant 有 tool_use 但没有结果时补 `is_error:true` 占位（与官方客户端一致）
/// - 相邻同 role 消息合并，首条保证是 user
pub fn to_anthropic_messages(messages: &[Value], system: &str) -> (Vec<Value>, String) {
    let mut out: Vec<Value> = Vec::new();
    let mut sys = system.to_string();
    // 最近一条 assistant 发出、尚未收到结果的 tool_use id 队列
    let mut pending_tool_ids: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    let mut synth_seq: u32 = 0;
    for m in messages {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
        // 非 tool 消息到来前，先把上一个 assistant 缺失的工具结果补齐
        if role != "tool" {
            synth_missing_tool_results(&mut out, &mut pending_tool_ids);
        }
        match role {
            "system" | "developer" => {
                let t = content_text(m.get("content"));
                if !t.is_empty() {
                    if !sys.is_empty() {
                        sys.push('\n');
                    }
                    sys.push_str(&t);
                }
            }
            "user" => {
                let blocks = content_blocks_of(m.get("content"), true);
                push_anthropic_blocks(&mut out, "user", blocks);
            }
            "assistant" => {
                let mut blocks = content_blocks_of(m.get("content"), false);
                if let Some(tcs) = m.get("tool_calls").and_then(|t| t.as_array()) {
                    for tc in tcs {
                        let name = tc
                            .pointer("/function/name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        if name.is_empty() {
                            continue;
                        }
                        let id = anthropic_tool_id(tc.get("id").and_then(|i| i.as_str()))
                            .unwrap_or_else(|| {
                                synth_seq += 1;
                                format!("toolu_{synth_seq:04}")
                            });
                        // tool_use.input 必须是 JSON 对象；arguments 是字符串时解析
                        let input = match tc.pointer("/function/arguments") {
                            Some(Value::String(s)) => {
                                let parsed: Value = serde_json::from_str(s).unwrap_or(Value::Null);
                                if parsed.is_object() { parsed } else { json!({}) }
                            }
                            Some(v) if v.is_object() => v.clone(),
                            _ => json!({}),
                        };
                        if !pending_tool_ids.contains(&id) {
                            pending_tool_ids.push_back(id.clone());
                        }
                        blocks.push(json!({ "type": "tool_use", "id": id, "name": name, "input": input }));
                    }
                }
                push_anthropic_blocks(&mut out, "assistant", blocks);
            }
            "tool" => {
                // tool_result 必须带 tool_use_id 且与紧邻 assistant 的 tool_use 对应：
                // 优先用消息里的 tool_call_id；缺失时按上游 tool_use 顺序补配；
                // id 对不上（孤儿结果）直接丢弃 —— 绝不发空 id/未知 id（2013 的直接触发原因）。
                let raw_id = anthropic_tool_id(m.get("tool_call_id").and_then(|i| i.as_str()));
                let id = match raw_id {
                    Some(id) if pending_tool_ids.iter().any(|x| x == &id) => {
                        pending_tool_ids.retain(|x| x != &id);
                        id
                    }
                    Some(_) => continue, // 孤儿 tool_result：上游不认识该 id
                    None => match pending_tool_ids.pop_front() {
                        Some(id) => id,
                        None => continue,
                    },
                };
                let text = content_text(m.get("content"));
                let block = if m.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false) {
                    json!({ "type": "tool_result", "tool_use_id": id, "content": text, "is_error": true })
                } else {
                    json!({ "type": "tool_result", "tool_use_id": id, "content": text })
                };
                push_tool_results(&mut out, vec![block]);
            }
            _ => {}
        }
    }
    // 收尾：最后的 assistant 仍缺工具结果 → 补齐
    synth_missing_tool_results(&mut out, &mut pending_tool_ids);
    // Anthropic 要求首条消息必须是 user（长上下文截断后可能以 assistant 开头）
    if out.first().and_then(|m| m.get("role")).and_then(|r| r.as_str()) == Some("assistant") {
        out.insert(
            0,
            json!({ "role": "user", "content": [{ "type": "text", "text": "(continue)" }] }),
        );
    }
    (out, sys)
}


/// 智谱开放平台 OpenAI 兼容端点
pub const ZCODE_CHAT_BASE: &str = "https://open.bigmodel.cn";
/// 智谱 chat 路径（OpenAI 兼容）
pub const ZCODE_CHAT_PATH: &str = "/api/paas/v4/chat/completions";
/// ZCode coding plan 走 Anthropic 兼容端点（asar 逆向：bigmodel/zai *CodingPlan / *StartPlan
/// provider 的 base = `${origin}/api/anthropic`，messages 路径 /v1/messages）
pub const ZCODE_ANTHROPIC_BASE: &str = "https://open.bigmodel.cn/api/anthropic";
pub const ZCODE_ANTHROPIC_PATH: &str = "/v1/messages";
/// ZCode Start Plan 对话端点（zcode.z.ai）
///
/// ⚠️ 该端点有服务端「仅官方客户端」闸门（`code=3012` / HTTP 405）。2026-11-19 实测闸门由
/// **两项**共同决定（见 [`zcode_start_plan_system_blocks`] 与 [`ZCODE_PLAN_HEADERS`]）：
/// ① 请求头 `X-ZCode-App-Version` 必须是当前客户端版本（3.18.2）；
/// ② 请求体 `system` 的前两个块必须是官方 CLI 的固定文本，且顺序不可颠倒。
/// 二者缺一即返回 3012。
pub const ZCODE_PLAN_ANTHROPIC_BASE: &str = "https://zcode.z.ai/api/v1/zcode-plan/anthropic";

/// ZCode Start Plan 请求头常量（2026-11-19 对官方 CLI 抓包逐项消融得出）。
///
/// 关键点：`X-ZCode-App-Version` 是 3012 闸门的**必要条件** —— 官方 CLI 自己发出的请求里
/// 该头是 `0.17.1`（AI SDK 默认值），因此官方 CLI 同样会被 3012 拒绝；把它改成当前版本
/// `3.18.2` 后同一请求立刻返回 200。
pub const ZCODE_PLAN_HEADERS: &[(&str, &str)] = &[
    ("anthropic-version", "2023-06-01"),
    ("http-referer", "https://zcode.z.ai"),
    ("user-agent", "ZCode/3.18.2 ai-sdk/provider-utils/4.0.27 runtime/node.js/24"),
    ("x-zcode-agent", "glm"),
    ("x-zcode-app-version", "3.18.2"),
    ("x-zcode-query-source", "main_turn"),
    ("x-zcode-session-type", "main"),
    ("x-title", "Z Code@electron"),
    ("x-platform", "win32-x64"),
    ("x-release-channel", "production"),
    ("x-client-language", "zh-CN"),
    ("x-client-timezone", "Asia/Shanghai"),
    ("x-os-category", "windows"),
    ("x-os-version", "10.0.26300"),
];

/// 官方 CLI 的身份前缀块（ZCode 3.18.2 `zcode.cjs` `buildCliPrefixSection` 逐字提取）。
pub const ZCODE_SYSTEM_CLI_PREFIX: &str = "You are ZCode, an interactive agent";

/// 官方 CLI 的身份 + 安全声明 + Harness 块（`buildIdentityPrompt` 逐字提取）。
///
/// 注意：起始有一个换行（`lJa` 用 `[["", identity, "", securityNotice].join("\n")` 拼接），
/// 且该文本参与服务端的客户端指纹比对 —— **不能改写、不能裁剪、不能调整顺序**。
pub const ZCODE_SYSTEM_IDENTITY: &str = "\nYou are an agent working with the user toward their goals, using your own judgment along the way.\n\nIMPORTANT: Assist with authorized security testing, defensive security, CTF challenges, and educational contexts. Refuse requests for destructive techniques, DoS attacks, mass targeting, supply chain compromise, or detection evasion for malicious purposes. Dual-use security tools (C2 frameworks, credential testing, exploit development) require clear authorization context: pentesting engagements, CTF competitions, security research, or defensive use cases.\n\n# Harness\n - Text you output outside of tool use is displayed to the user as Github-flavored markdown in a terminal.\n - Tools run behind a user-selected permission mode; a denied call means the user declined it — adjust, don't retry verbatim.\n - `<system-reminder>` tags in messages and tool results are injected by the harness, not the user. Hooks may intercept tool calls; treat hook output as user feedback.\n - Prefer the dedicated file/search tools over shell commands when one fits. Independent tool calls can run in parallel in one response.\n - Reference code as `file_path:line_number` — it's clickable.";

/// 构造 Start Plan 端点所需的 `system` 数组（Anthropic 文本块 + cache_control）。
///
/// 实测（2026-11-19，逐一消融）：
/// - 缺 `system` → 3012；只有第 0 块 → 3012；只有第 1 块 → 3012；
/// - 第 0 块 + 第 1 块 → **200**；顺序颠倒 → 3012；第 0 块文本前加空格 → 3012；
/// - 改写第 1 块个别词 → 200（说明校验的是「块数 + 前缀 + 结构」，不是全文逐字哈希）。
///
/// 因此最少需要前两块；用户自己的 system 提示词作为第 3 块追加。
pub fn zcode_start_plan_system_blocks(user_system: &str) -> Vec<Value> {
    let mut blocks = vec![
        json!({ "type": "text", "text": ZCODE_SYSTEM_CLI_PREFIX, "cache_control": { "type": "ephemeral" } }),
        json!({ "type": "text", "text": ZCODE_SYSTEM_IDENTITY, "cache_control": { "type": "ephemeral" } }),
    ];
    if !user_system.trim().is_empty() {
        blocks.push(json!({ "type": "text", "text": user_system, "cache_control": { "type": "ephemeral" } }));
    }
    blocks
}

/// ZCode Anthropic 格式 body（coding plan 通道）。`messages` 须已经过
/// `to_anthropic_messages` 转换（Anthropic 线格式），此处只做打包。
pub fn zcode_anthropic_body(messages: &[Value], system: &str, model: &str, max_tokens: Option<u32>) -> Value {
    let mut body = json!({
        "model": model,
        "messages": messages,
        "max_tokens": max_tokens.unwrap_or(4096),
        "stream": true,
    });
    if !system.is_empty() {
        body["system"] = json!(system);
    }
    body
}

/// ZCode chat 请求头：凭据即智谱 access_token；deviceMid 可选
pub fn zcode_chat_headers(token: &str, device_mid: Option<&str>) -> Vec<(String, String)> {
    // 逐字对齐 ZCode 官方客户端 buildZCodeSourceHeadersFromContext（asar 逆向）
    let mut hs = vec![
        ("Authorization".to_string(), format!("Bearer {token}")),
        ("Content-Type".to_string(), "application/json".to_string()),
        ("Accept".to_string(), "application/json".to_string()),
        ("User-Agent".to_string(), "ZCode/3.18.2".to_string()),
        ("HTTP-Referer".to_string(), "https://zcode.z.ai".to_string()),
        ("X-Title".to_string(), "Z Code@electron".to_string()),
        ("X-ZCode-App-Version".to_string(), "3.18.2".to_string()),
        ("X-Platform".to_string(), "win32-x64".to_string()),
        ("X-Client-Language".to_string(), "zh-CN".to_string()),
        ("X-Client-Timezone".to_string(), "Asia/Shanghai".to_string()),
        ("X-Os-Category".to_string(), "windows".to_string()),
    ];
    if let Some(mid) = device_mid {
        if !mid.is_empty() {
            // 官方头是 X-Device-Mid（网关旧实现误用 X-Device-Id）
            hs.push(("X-Device-Mid".to_string(), mid.to_string()));
        }
    }
    hs
}

/// ZCode Start Plan 端点请求头（2026-11-19 对官方 CLI 抓包 + 逐项消融得出）。
///
/// `X-ZCode-App-Version` 必须与当前客户端版本一致（3.18.2），否则服务端返回 3012。
/// 认证用本机 `zcodejwttoken`（长效会话 JWT），同时带 `X-Api-Key`（官方 AI SDK 行为）。
pub fn zcode_plan_chat_headers(token: &str, device_mid: &str) -> Vec<(String, String)> {
    let mut hs: Vec<(String, String)> = ZCODE_PLAN_HEADERS
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    hs.push(("authorization".to_string(), format!("Bearer {token}")));
    hs.push(("x-api-key".to_string(), token.to_string()));
    hs.push(("content-type".to_string(), "application/json".to_string()));
    if !device_mid.is_empty() {
        hs.push(("x-device-mid".to_string(), device_mid.to_string()));
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

/// MiniMax chat 请求头（凭据：MMOAT OAuth token 或 API key）。
/// 对齐 MiniMax Code 官方客户端（pi-ai anthropic provider）：
/// - Authorization Bearer 是真正鉴权；x-api-key 官方是占位
/// - X-Mavis-Session-Id 按 token 稳定（影响服务端缓存亲和），Agent-Id 用固定 main
/// - 时区偏移按秒（UTC+8 = 28800）
pub fn minimax_chat_headers(token: &str) -> Vec<(String, String)> {
    use sha2::{Digest, Sha256};
    let session_id = {
        let h = Sha256::digest(token.as_bytes());
        let hex: String = h.iter().take(16).map(|b| format!("{b:02x}")).collect();
        format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
    };
    vec![
        ("Authorization".to_string(), format!("Bearer {token}")),
        ("x-api-key".to_string(), token.to_string()),
        ("anthropic-version".to_string(), "2023-06-01".to_string()),
        ("Content-Type".to_string(), "application/json".to_string()),
        ("Accept".to_string(), "application/json".to_string()),
        ("User-Agent".to_string(), "MiniMaxAgent".to_string()),
        ("X-Mavis-Session-Id".to_string(), session_id),
        ("X-Mavis-Agent-Id".to_string(), "main".to_string()),
        ("X-Mavis-Timezone-Offset".to_string(), "28800".to_string()),
        ("X-Mavis-Turn-Phase".to_string(), "initial".to_string()),
    ]
}

/// MiniMax Anthropic Messages body（Anthropic 格式与 OpenAI 不同：system 是顶层字段）
pub fn minimax_chat_body(messages: &[Value], system: &str, model: &str, max_tokens: Option<u32>) -> Value {
    // OpenAI 线格式 → Anthropic 线格式（tool_use / tool_result 必须按 Anthropic 规范成对出现，
    // 否则上游直接 400：tool result's tool id() not found (2013)）
    let (msgs, sys) = to_anthropic_messages(messages, system);
    let mut body = json!({
        "model": model,
        "messages": msgs,
        "max_tokens": max_tokens.unwrap_or(4096),
        "stream": true,
    });
    if !sys.is_empty() {
        body["system"] = json!(sys);
    }
    body
}

/// 解析 Anthropic SSE chunk → 事件列表（MiniMax/ZCode Anthropic 兼容通道返回 Anthropic 线格式）。
///
/// `block_tool_index` / `next_tool_index` 维护「Anthropic content block index → OpenAI tool_calls index」
/// 映射：工具块用块序号（文本块 0、工具块 1…），客户端按 tool_calls[].index 聚合参数分片。
pub fn anthropic_chunk_with_map(
    v: &Value,
    block_tool_index: &mut std::collections::HashMap<u64, u32>,
    next_tool_index: &mut u32,
) -> Vec<crate::upstream::StreamEvent> {
    use crate::upstream::StreamEvent;
    let mut out = Vec::new();
    let typ = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match typ {
        "error" => {
            let err = v.get("error").cloned().unwrap_or(Value::Null);
            let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("upstream error").to_string();
            let code = err.get("type").and_then(|t| t.as_str()).unwrap_or("anthropic_error").to_string();
            out.push(StreamEvent::Error(msg, code));
        }
        "content_block_start" => {
            let idx = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
            let block = v.get("content_block").cloned().unwrap_or(Value::Null);
            match block.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "tool_use" => {
                    let tool_index = *next_tool_index;
                    *next_tool_index += 1;
                    block_tool_index.insert(idx, tool_index);
                    let id = block.get("id").and_then(|i| i.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string());
                    let name = block.get("name").and_then(|n| n.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string());
                    // 部分上游会把初始 input 放在 content_block_start 里（通常是 {}）
                    let args = block.get("input").filter(|i| i.is_object() && i.as_object().map(|o| !o.is_empty()).unwrap_or(false)).map(|i| i.to_string());
                    if id.is_some() || name.is_some() || args.is_some() {
                        out.push(StreamEvent::ToolCall { index: tool_index, id, name, arguments: args });
                    }
                }
                "text" => {
                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                        if !t.is_empty() {
                            out.push(StreamEvent::Text(t.to_string()));
                        }
                    }
                }
                _ => {}
            }
        }
        "content_block_delta" => {
            let idx = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
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
                "input_json_delta" => {
                    // 工具参数增量：Anthropic 用 partial_json 字符串分片（与 OpenAI arguments 分片同语义）
                    if let Some(pj) = delta.get("partial_json").and_then(|p| p.as_str()) {
                        if !pj.is_empty() {
                            let tool_index = block_tool_index.get(&idx).copied().unwrap_or(0);
                            out.push(StreamEvent::ToolCall { index: tool_index, id: None, name: None, arguments: Some(pj.to_string()) });
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
                if input > 0 || output > 0 {
                    out.push(StreamEvent::Usage { input, output, total: Some(input + output) });
                }
            }
            if let Some(sr) = v.get("delta").and_then(|d| d.get("stop_reason")).and_then(|s| s.as_str()) {
                // Anthropic stop_reason → OpenAI finish_reason
                let finish = match sr {
                    "tool_use" => "tool_calls",
                    "max_tokens" => "length",
                    _ => "stop",
                };
                out.push(StreamEvent::Done { finish: finish.to_string() });
            }
        }
        "message_stop" => {
            // MiniMax mavis 通道以 message_stop 结束且可能不带 message_delta/stop_reason
            out.push(StreamEvent::Done { finish: "stop".into() });
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
        "ping" => {}
        _ => {}
    }
    out
}

/// 解析 Anthropic SSE chunk（无状态便捷入口；工具块 index 即 content block index）
pub fn parse_anthropic_chunk(v: &Value) -> Vec<crate::upstream::StreamEvent> {
    let mut map = std::collections::HashMap::new();
    let mut next = 0u32;
    anthropic_chunk_with_map(v, &mut map, &mut next)
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

// ============================ CodeArts（华为码道）============================

/// 码道 chat 端点（product.json inferhubBaseUrl；主域失败 fallback .cn 域）
pub const CODEARTS_CHAT_BASE: &str = "https://snap-access.cn-north-4.myhuaweicloud.com";
pub const CODEARTS_CHAT_BASE_FALLBACK: &str = "https://snap-access.cn-north-4.myhuaweicloud.cn";
/// kernel 头白名单 maas_type 用（benefit/education 渠道），默认对话不带
pub const CODEARTS_CLIENT_VERSION: &str = "0.2.9";

/// 码道 chat body：OpenAI 兼容 + stream_options.include_usage + enable_thinking
/// （kernel chat_core.rs 逆向：无 sessionId/agentId，多轮靠完整 messages 重发）
pub fn codearts_chat_body(messages: &[Value], system: &str, model: &str, max_tokens: Option<u32>, reasoning_effort: Option<&str>) -> Value {
    let mut msgs: Vec<Value> = Vec::new();
    if !system.is_empty() {
        msgs.push(json!({ "role": "system", "content": system }));
    }
    for m in messages {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user").to_string();
        // content 展平为字符串（OpenAI 线格式 string content）
        let content = match m.get("content") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(arr)) => arr.iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>().join("\n"),
            _ => String::new(),
        };
        if role == "system" && !system.is_empty() { continue; } // 已注入首条
        msgs.push(json!({ "role": role, "content": content }));
    }
    let mut body = json!({
        "model": model,
        "messages": msgs,
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    if let Some(mt) = max_tokens {
        body["max_tokens"] = json!(mt);
    }
    // enable_thinking：off/none 关闭，其余开启（kernel fast-model 注入逻辑）
    body["enable_thinking"] = json!(!matches!(reasoning_effort, Some("off") | Some("none")));
    body
}

// 码道 chat 请求头在 upstream::codearts_chat_stream 内构造（需要 Cred 全量字段）
// 此处仅保留 body 构造。

// ============================ 工具调用（function calling）透传 ============================

/// OpenAI 风格透传：tools 原样 + tool_choice（codearts / workbuddy / qoder / zcode 普通 OpenAI 端点）
pub fn attach_openai_tools(body: &mut Value, tools: &[Value], tool_choice: Option<&Value>) {
    if tools.is_empty() { return; }
    body["tools"] = json!(tools);
    if let Some(tc) = tool_choice {
        // OpenAI: "auto" | "none" | "required" | {type:"function", function:{name}}
        body["tool_choice"] = tc.clone();
    }
}

/// Anthropic 风格转换：{name, description, input_schema} + tool_choice {type:auto|any|tool}
pub fn attach_anthropic_tools(body: &mut Value, tools: &[Value], tool_choice: Option<&Value>) {
    if tools.is_empty() { return; }
    let converted: Vec<Value> = tools.iter().filter_map(|t| {
        let f = t.get("function")?;
        let name = f.get("name").and_then(|n| n.as_str())?;
        let mut out = json!({
            "name": name,
            "input_schema": f.get("parameters").cloned().unwrap_or(json!({ "type": "object", "properties": {} })),
        });
        // description 缺省时省略（官方客户端不发送 null description）
        if let Some(d) = f.get("description").and_then(|d| d.as_str()).filter(|s| !s.is_empty()) {
            out["description"] = json!(d);
        }
        Some(out)
    }).collect();
    if converted.is_empty() { return; }
    body["tools"] = json!(converted);
    if let Some(tc) = tool_choice {
        let mapped = match tc {
            Value::String(s) if s == "auto" => json!({ "type": "auto" }),
            Value::String(s) if s == "none" => Value::Null,
            Value::String(s) if s == "required" => json!({ "type": "any" }),
            Value::Object(o) => {
                let name = o.get("function").and_then(|f| f.get("name")).and_then(|n| n.as_str()).unwrap_or("");
                if name.is_empty() { json!({ "type": "auto" }) } else { json!({ "type": "tool", "name": name }) }
            }
            _ => json!({ "type": "auto" }),
        };
        if !mapped.is_null() { body["tool_choice"] = mapped; }
    }
}

/// Trae SOLO：tools[].function.parameters 需要是 **JSON 字符串**（官方 prepareSoloBody 行为）
pub fn attach_trae_tools(body: &mut Value, tools: &[Value]) {
    if tools.is_empty() { return; }
    let converted: Vec<Value> = tools.iter().map(|t| {
        let mut t = t.clone();
        if let Some(params) = t.pointer_mut("/function/parameters") {
            if !matches!(params, Value::String(_)) {
                *params = json!(params.to_string());
            }
        }
        t
    }).collect();
    body["tools"] = json!(converted);
}

