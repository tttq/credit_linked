//! 上游调用层：模型目录抓取 + chat SSE 流式转发。
//!
//! 所有请求直接用凭据打上游；chat 一律流式（WorkBuddy 上游拒绝非流式，
//! Trae SOLO 通道也是 stream-only），非流式响应由网关层聚合。

use std::time::Duration;

use futures_core::Stream;
use serde_json::{json, Value};

use credit_core::adapters::Cred;
use credit_core::error::PlatformError;

use crate::protocol::*;

/// 上游 chat 产出的一条流事件
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// 正文增量
    Text(String),
    /// 思考增量
    Reasoning(String),
    /// token 用量
    Usage { input: u64, output: u64, total: Option<u64> },
    /// 正常结束
    Done { finish: String },
    /// 上游错误（message, code）
    Error(String, String),
}

/// 平台无关的模型条目
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credit_multiplier: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_images: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_tool_call: Option<bool>,
}

fn http_client() -> &'static reqwest::Client {
    static C: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .connect_timeout(Duration::from_secs(20))
            .build()
            .expect("gateway http client")
    })
}

// ============================ WorkBuddy ============================

/// WorkBuddy 模型目录：GET {chat_base}/v3/config，code!=0 信封，data.models × agents(cli)
pub async fn wb_fetch_models(cred: &Cred) -> Result<Vec<ModelInfo>, PlatformError> {
    let domain = cred.meta.get("domain").and_then(|d| d.as_str()).unwrap_or("");
    let global = wb_is_global(domain);
    let base = if global { wb_chat_base(domain) } else { WB_CN_CHAT_BASE };
    let url = format!("{base}/v3/config");
    let uid = &cred.uid;
    let enterprise = cred.meta.get("enterpriseId").and_then(|e| e.as_str()).unwrap_or("");
    let hs = wb_config_headers(&cred.token, uid, domain, global);
    let mut req = http_client().get(&url);
    for (k, v) in &hs {
        req = req.header(k, v);
    }
    let _ = enterprise;
    let resp = req.send().await.map_err(|e| PlatformError::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.map_err(|e| PlatformError::Format(e.to_string()))?;
    if status != 200 {
        return Err(PlatformError::Format(format!("v3/config HTTP {status}")));
    }
    let code = body.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code != 0 {
        return Err(PlatformError::business_of(&body));
    }
    let data = body.get("data").cloned().unwrap_or(Value::Null);
    let models = data.get("models").and_then(|m| m.as_array()).cloned().unwrap_or_default();
    // cli 代理桶（决定哪些模型暴露给 CLI 通道）；无桶时全量暴露
    let cli_ids: Option<std::collections::HashSet<String>> = data.get("agents").and_then(|a| a.as_array()).and_then(|agents| {
        agents.iter().find(|ag| ag.get("id").and_then(|i| i.as_str()) == Some("cli"))
            .and_then(|cli| cli.get("models"))
            .and_then(|m| m.as_array())
            .map(|arr| arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
    });
    let mut out = Vec::new();
    for m in models {
        let id = m.get("id").and_then(|i| i.as_str()).unwrap_or_default().to_string();
        if id.is_empty() || m.get("disabled").and_then(|d| d.as_bool()).unwrap_or(false) {
            continue;
        }
        // auto 路由不算模型
        if id == "auto" {
            continue;
        }
        if let Some(set) = &cli_ids {
            if !set.contains(&id) {
                continue;
            }
        }
        let input = m.get("maxInputTokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let output = m.get("maxOutputTokens").and_then(|v| v.as_u64()).unwrap_or(0);
        if input == 0 || output == 0 {
            continue;
        }
        out.push(ModelInfo {
            name: m.get("name").and_then(|n| n.as_str()).filter(|s| !s.is_empty()).unwrap_or(&id).to_string(),
            id,
            context_window: Some(input),
            max_tokens: Some(output),
            credit_multiplier: m.pointer("/credits/base").and_then(|v| v.as_f64()),
            supports_images: m.get("supportsImages").and_then(|v| v.as_bool()),
            supports_tool_call: m.get("supportsToolCall").and_then(|v| v.as_bool()),
        });
    }
    Ok(out)
}

/// WorkBuddy chat：POST {chat_base}/v2/chat/completions（OpenAI 线格式 SSE）
pub async fn wb_chat_stream(
    cred: &Cred,
    body: Value,
) -> Result<impl Stream<Item = Result<StreamEvent, String>>, String> {
    let domain = cred.meta.get("domain").and_then(|d| d.as_str()).unwrap_or("");
    let base = if wb_is_global(domain) { wb_chat_base(domain) } else { WB_CN_CHAT_BASE };
    let url = format!("{base}/v2/chat/completions");
    let enterprise = cred.meta.get("enterpriseId").and_then(|e| e.as_str());
    let hs = wb_chat_headers(&cred.token, &cred.uid, domain, enterprise);
    let mut req = http_client().post(&url).json(&body);
    for (k, v) in &hs {
        req = req.header(k, v);
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("upstream HTTP {status}: {}", truncate(&text, 400)));
    }
    Ok(parse_openai_sse(resp.bytes_stream()))
}

/// OpenAI 线格式 SSE → StreamEvent 流
fn parse_openai_sse(
    stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>>,
) -> impl Stream<Item = Result<StreamEvent, String>> {
    futures_util::stream::try_unfold(
        (stream, SseBuffer::default()),
        |(mut stream, mut buf)| async move {
            loop {
                if let Some(ev) = buf.next_event() {
                    return Ok(Some((ev, (stream, buf))));
                }
                match stream.next().await {
                    Some(Ok(chunk)) => buf.feed(&chunk),
                    Some(Err(e)) => return Err(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())),
                    None => {
                        let tail = buf.take_tail();
                        return Ok::<_, std::io::Error>(tail.map(|ev| (ev, (stream, buf))));
                    }
                }
            }
        },
    )
    .map_err(|e| e.to_string())
}

fn parse_openai_chunk(v: &Value) -> Vec<StreamEvent> {
    let mut out = Vec::new();
    // OpenAI 错误体
    if let Some(err) = v.get("error") {
        let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("upstream error").to_string();
        let code = err.get("code").and_then(|c| c.as_str()).unwrap_or("upstream_error").to_string();
        out.push(StreamEvent::Error(msg, code));
        return out;
    }
    // WorkBuddy 队列/错误信封 {"code":6020,...}
    if let Some(code) = v.get("code").and_then(|c| c.as_i64()) {
        if code != 0 && v.get("choices").is_none() {
            let msg = v.get("message").or_else(|| v.get("msg")).and_then(|m| m.as_str()).unwrap_or("upstream refused").to_string();
            out.push(StreamEvent::Error(msg, code.to_string()));
            return out;
        }
    }
    if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
        for ch in choices {
            let delta = ch.get("delta").cloned().unwrap_or(Value::Null);
            if let Some(rc) = delta.get("reasoning_content").and_then(|r| r.as_str()) {
                if !rc.is_empty() {
                    out.push(StreamEvent::Reasoning(rc.to_string()));
                }
            }
            if let Some(rc) = delta.get("reasoning").and_then(|r| r.as_str()) {
                if !rc.is_empty() {
                    out.push(StreamEvent::Reasoning(rc.to_string()));
                }
            }
            if let Some(c) = delta.get("content").and_then(|c| c.as_str()) {
                if !c.is_empty() {
                    out.push(StreamEvent::Text(c.to_string()));
                }
            }
            if let Some(fr) = ch.get("finish_reason").and_then(|f| f.as_str()) {
                if !fr.is_empty() {
                    out.push(StreamEvent::Done { finish: fr.to_string() });
                }
            }
        }
    }
    if let Some(u) = v.get("usage") {
        let input = u.get("prompt_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
        let output = u.get("completion_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
        let total = u.get("total_tokens").and_then(|t| t.as_u64());
        out.push(StreamEvent::Usage { input, output, total });
    }
    out
}

// ============================ Trae ============================

/// Trae 模型目录：GET {remote}/models?functions=...（data.list[]）
pub async fn trae_fetch_models(cred: &Cred, intl: bool) -> Result<Vec<ModelInfo>, PlatformError> {
    let base = if intl { TRAE_AI_REMOTE_BASE } else { TRAE_CN_REMOTE_BASE };
    let functions = if intl {
        TRAE_DIRECTORY_FUNCTIONS_AI.join(",")
    } else {
        TRAE_DIRECTORY_FUNCTIONS_CN.join(",")
    };
    let url = format!("{base}/models?functions={functions}");
    let hs = trae_remote_headers(&cred.token, intl);
    let mut req = http_client().get(&url);
    for (k, v) in &hs {
        req = req.header(k, v);
    }
    let resp = req.send().await.map_err(|e| PlatformError::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.map_err(|e| PlatformError::Format(e.to_string()))?;
    if status != 200 {
        return Err(PlatformError::Format(format!("remote models HTTP {status}")));
    }
    let list = body.pointer("/data/list").and_then(|l| l.as_array()).cloned().unwrap_or_default();
    let mut out = Vec::new();
    for row in list {
        let id = row.get("name").and_then(|n| n.as_str()).unwrap_or_default().to_string();
        if id.is_empty() {
            continue;
        }
        let context = row.get("context_window_tokens").cloned().unwrap_or(Value::Null);
        let ctx = context.get("dev").and_then(|v| v.as_u64());
        let max_ctx = if row.get("max_mode").and_then(|v| v.as_bool()).unwrap_or(false) {
            context.get("max").and_then(|v| v.as_u64())
        } else {
            None
        };
        let features = row.get("features").cloned().unwrap_or(Value::Null);
        let multimodal = row.get("multimodal").and_then(|v| v.as_bool());
        let consumption_rate = features.pointer("/consumption_rate/data/rate").and_then(|v| v.as_f64());
        out.push(ModelInfo {
            name: row.get("display_name").and_then(|n| n.as_str()).filter(|s| !s.is_empty()).unwrap_or(&id).to_string(),
            id,
            context_window: ctx,
            max_tokens: max_ctx,
            credit_multiplier: consumption_rate,
            supports_images: multimodal,
            supports_tool_call: None,
        });
    }
    Ok(out)
}

/// Trae chat：POST {chat}/api/agent/v3/llm_utils_chat（自定义 SSE 事件流）
pub async fn trae_chat_stream(
    cred: &Cred,
    intl: bool,
    enterprise: bool,
    body: Value,
) -> Result<impl Stream<Item = Result<StreamEvent, String>>, String> {
    let base = if enterprise {
        TRAE_ENTERPRISE_HOST.to_string()
    } else if intl {
        TRAE_AI_CHAT_BASE.to_string()
    } else {
        TRAE_CN_CHAT_BASE.to_string()
    };
    let url = format!("{base}/api/agent/v3/llm_utils_chat");
    let hs = trae_chat_headers(&cred.token, &cred.meta);
    let mut req = http_client().post(&url).json(&body);
    for (k, v) in &hs {
        req = req.header(k, v);
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        let text = resp.text().await.unwrap_or_default();
        // 企业网关拒绝也可能是 200+首帧 event:error，这里只拦真 HTTP 错
        let code = if status == 400 || status == 401 || status == 403 { "auth" } else { "upstream" };
        return Err(format!("upstream HTTP {status} [{code}]: {}", truncate(&text, 400)));
    }
    Ok(parse_trae_sse(resp.bytes_stream()))
}

/// Trae 自定义 SSE → StreamEvent 流（对齐 decodeTraeEvent）
fn parse_trae_sse(
    stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>>,
) -> impl Stream<Item = Result<StreamEvent, String>> {
    futures_util::stream::try_unfold(
        (stream, TraeSseBuffer::default()),
        |(mut stream, mut buf)| async move {
            loop {
                if let Some(ev) = buf.pop_event() {
                    return Ok(Some((ev, (stream, buf))));
                }
                match stream.next().await {
                    Some(Ok(chunk)) => buf.feed(&chunk),
                    Some(Err(e)) => return Err(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())),
                    None => {
                        let tail = buf.take_tail_event();
                        return Ok::<_, std::io::Error>(tail.map(|ev| (ev, (stream, buf))));
                    }
                }
            }
        },
    )
    .map_err(|e| e.to_string())
}

/// decodeTraeEvent 对齐：event 名 + data JSON → StreamEvent
fn decode_trae_event(event_name: &str, data: &str) -> Option<StreamEvent> {
    if data == "[DONE]" {
        return Some(StreamEvent::Done { finish: "stop".into() });
    }
    let Ok(v) = serde_json::from_str::<Value>(data) else {
        return None;
    };
    match event_name {
        "request_wait_in_queue" => {
            let pos = v.get("position").and_then(|p| p.as_u64()).unwrap_or(0);
            Some(StreamEvent::Error(format!("上游排队中（第 {pos} 位），请稍后重试"), "queue".into()))
        }
        "progress_notice" => None,
        "token_usage" => Some(StreamEvent::Usage {
            input: v.get("prompt_tokens").and_then(|t| t.as_u64()).unwrap_or(0),
            output: v.get("completion_tokens").and_then(|t| t.as_u64()).unwrap_or(0),
            total: v.get("total_tokens").and_then(|t| t.as_u64()),
        }),
        "metadata" | "timing_cost" => None,
        "done" => Some(StreamEvent::Done {
            finish: v.get("finish_reason").and_then(|f| f.as_str()).unwrap_or("stop").to_string(),
        }),
        "error" => {
            let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
            let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string();
            let readable = match code {
                1005 => "该模型需要付费套餐".to_string(),
                4011 | 4001 => "该账号已被限频或模型不在套餐内（可换账号/模型重试）".to_string(),
                _ => format!("上游错误 code={code}: {msg}"),
            };
            Some(StreamEvent::Error(readable, code.to_string()))
        }
        "output" => {
            let mut out = Vec::new();
            if let Some(rc) = v.get("reasoning_content").and_then(|r| r.as_str()) {
                if !rc.is_empty() {
                    out.push(StreamEvent::Reasoning(rc.to_string()));
                }
            }
            if let Some(c) = v.get("response").and_then(|r| r.as_str()) {
                if !c.is_empty() {
                    out.push(StreamEvent::Text(c.to_string()));
                }
            }
            out.into_iter().next()
        }
        _ => {
            // 未命名事件也可能带 response
            if v.get("response").and_then(|r| r.as_str()).is_some() || v.get("reasoning_content").is_some() {
                let mut out = Vec::new();
                if let Some(rc) = v.get("reasoning_content").and_then(|r| r.as_str()) {
                    if !rc.is_empty() {
                        out.push(StreamEvent::Reasoning(rc.to_string()));
                    }
                }
                if let Some(c) = v.get("response").and_then(|r| r.as_str()) {
                    if !c.is_empty() {
                        out.push(StreamEvent::Text(c.to_string()));
                    }
                }
                out.into_iter().next()
            } else if v.get("finish_reason").is_some() {
                Some(StreamEvent::Done {
                    finish: v.get("finish_reason").and_then(|f| f.as_str()).unwrap_or("stop").to_string(),
                })
            } else {
                None
            }
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n])
    }
}

// ============================ SSE 缓冲 ============================

/// OpenAI 风格 SSE 缓冲：data: 行 → 事件队列（一帧可展开多条事件）
#[derive(Default)]
struct SseBuffer {
    buf: Vec<u8>,
    queue: std::collections::VecDeque<StreamEvent>,
}

impl SseBuffer {
    fn feed(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }
    /// 取下一条事件：队列优先，否则解析下一个 data: 帧
    fn next_event(&mut self) -> Option<StreamEvent> {
        if let Some(ev) = self.queue.pop_front() {
            return Some(ev);
        }
        loop {
            let frame = self.next_frame()?;
            if frame == "[DONE]" {
                return Some(StreamEvent::Done { finish: "stop".into() });
            }
            if let Ok(v) = serde_json::from_str::<Value>(&frame) {
                let events = parse_openai_chunk(&v);
                if events.is_empty() {
                    continue;
                }
                let mut it = events.into_iter();
                let first = it.next().unwrap();
                self.queue.extend(it);
                return Some(first);
            }
        }
    }
    /// 流结束后残余帧
    fn take_tail(&mut self) -> Option<StreamEvent> {
        if let Some(ev) = self.queue.pop_front() {
            return Some(ev);
        }
        if self.buf.is_empty() {
            return None;
        }
        let block = String::from_utf8_lossy(&self.buf).to_string();
        self.buf.clear();
        for line in block.lines() {
            if let Some(data) = line.strip_prefix("data:") {
                let d = data.trim();
                if d.is_empty() {
                    continue;
                }
                if d == "[DONE]" {
                    return Some(StreamEvent::Done { finish: "stop".into() });
                }
                if let Ok(v) = serde_json::from_str::<Value>(d) {
                    let events = parse_openai_chunk(&v);
                    if !events.is_empty() {
                        let mut it = events.into_iter();
                        let first = it.next().unwrap();
                        self.queue.extend(it);
                        return Some(first);
                    }
                }
            }
        }
        None
    }
    /// 内部：取下一个 data: 帧
    fn next_frame(&mut self) -> Option<String> {
        loop {
            let pos = find_double_newline(&self.buf)?;
            let block = String::from_utf8_lossy(&self.buf[..pos]).to_string();
            self.buf.drain(..pos + 2);
            for line in block.lines() {
                if let Some(data) = line.strip_prefix("data:") {
                    let d = data.trim();
                    if d.is_empty() {
                        continue;
                    }
                    return Some(d.to_string());
                }
            }
        }
    }
}

/// Trae 事件缓冲（event: + data: 成对）
#[derive(Default)]
struct TraeSseBuffer {
    buf: Vec<u8>,
    queue: std::collections::VecDeque<StreamEvent>,
}

type TraeEvent = (String, String);

impl TraeSseBuffer {
    fn feed(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }
    fn pop_event(&mut self) -> Option<StreamEvent> {
        if let Some(ev) = self.queue.pop_front() {
            return Some(ev);
        }
        loop {
            let (name, data) = self.next_event()?;
            if let Some(ev) = decode_trae_event(&name, &data) {
                return Some(ev);
            }
        }
    }
    fn next_event(&mut self) -> Option<TraeEvent> {
        loop {
            let pos = find_double_newline(&self.buf)?;
            let block = String::from_utf8_lossy(&self.buf[..pos]).to_string();
            self.buf.drain(..pos + 2);
            if let Some(e) = parse_event_block(&block) {
                return Some(e);
            }
        }
    }
    fn take_tail_event(&mut self) -> Option<StreamEvent> {
        if let Some(ev) = self.queue.pop_front() {
            return Some(ev);
        }
        if self.buf.is_empty() {
            return None;
        }
        let block = String::from_utf8_lossy(&self.buf).to_string();
        self.buf.clear();
        let (name, data) = parse_event_block(&block)?;
        decode_trae_event(&name, &data)
    }
}

fn find_double_newline(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n").or_else(|| {
        buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 2)
    })
}

fn parse_event_block(block: &str) -> Option<TraeEvent> {
    let mut event_name = String::new();
    let mut data = String::new();
    for line in block.lines() {
        if let Some(e) = line.strip_prefix("event:") {
            event_name = e.trim().to_string();
        } else if let Some(d) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(d.trim_start_matches(' '));
        }
    }
    if data.is_empty() && event_name.is_empty() {
        return None;
    }
    Some((event_name, data))
}
