//! 上游调用层：模型目录抓取 + chat SSE 流式转发。
//!
//! chat 一律流式（WorkBuddy 上游拒绝非流式，Trae SOLO 通道也是 stream-only），
//! 非流式响应由网关层聚合。SSE 解析用统一的"泵"结构：拉 chunk → 行缓冲 → 逐条吐事件。

use std::time::Duration;

use futures_core::Stream;
use futures_util::StreamExt;
use serde_json::Value;

use credit_core::adapters::Cred;
use credit_core::error::PlatformError;

use crate::protocol::*;

/// 上游 chat 产出的一条流事件
#[derive(Debug, Clone)]
pub enum StreamEvent {
    Text(String),
    Reasoning(String),
    Usage { input: u64, output: u64, total: Option<u64> },
    Done { finish: String },
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
}

pub type EventStream = std::pin::Pin<Box<dyn Stream<Item = Result<StreamEvent, String>> + Send>>;

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

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n])
    }
}

async fn send_sse(req: reqwest::RequestBuilder) -> Result<reqwest::Response, String> {
    let resp = req.send().await.map_err(|e| format!("network: {e}"))?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("upstream HTTP {status}: {}", truncate(&text, 400)));
    }
    Ok(resp)
}

// ============================ WorkBuddy ============================

/// WorkBuddy 模型目录：GET {chat_base}/v3/config
pub async fn wb_fetch_models(cred: &Cred) -> Result<Vec<ModelInfo>, PlatformError> {
    let domain = cred.meta.get("domain").and_then(|d| d.as_str()).unwrap_or("");
    let global = wb_is_global(domain);
    let base = if global { wb_chat_base(domain) } else { WB_CN_CHAT_BASE };
    let url = format!("{base}/v3/config");
    let hs = wb_config_headers(&cred.token, &cred.uid, domain, global);
    let mut req = http_client().get(&url);
    for (k, v) in &hs {
        req = req.header(k, v);
    }
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
    let cli_ids: Option<std::collections::HashSet<String>> = data.get("agents").and_then(|a| a.as_array()).and_then(|agents| {
        agents.iter().find(|ag| ag.get("id").and_then(|i| i.as_str()) == Some("cli"))
            .and_then(|cli| cli.get("models"))
            .and_then(|m| m.as_array())
            .map(|arr| arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
    });
    let mut out = Vec::new();
    for m in models {
        let id = m.get("id").and_then(|i| i.as_str()).unwrap_or_default().to_string();
        if id.is_empty() || id == "auto" || m.get("disabled").and_then(|d| d.as_bool()).unwrap_or(false) {
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
        });
    }
    Ok(out)
}

/// WorkBuddy chat：POST {chat_base}/v2/chat/completions（OpenAI 线格式 SSE）
pub async fn wb_chat_stream(cred: &Cred, body: Value) -> Result<EventStream, String> {
    let domain = cred.meta.get("domain").and_then(|d| d.as_str()).unwrap_or("");
    let base = if wb_is_global(domain) { wb_chat_base(domain) } else { WB_CN_CHAT_BASE };
    let url = format!("{base}/v2/chat/completions");
    let enterprise = cred.meta.get("enterpriseId").and_then(|e| e.as_str());
    let hs = wb_chat_headers(&cred.token, &cred.uid, domain, enterprise);
    let mut req = http_client().post(&url).json(&body);
    for (k, v) in &hs {
        req = req.header(k, v);
    }
    let resp = send_sse(req).await?;
    Ok(parse_openai_sse(resp.bytes_stream()))
}

// ============================ Trae ============================

/// Trae 模型目录：GET {remote}/models?functions=...
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
        let rate = features.pointer("/consumption_rate/data/rate").and_then(|v| v.as_f64());
        out.push(ModelInfo {
            name: row.get("display_name").and_then(|n| n.as_str()).filter(|s| !s.is_empty()).unwrap_or(&id).to_string(),
            id,
            context_window: ctx,
            max_tokens: max_ctx,
            credit_multiplier: rate,
            supports_images: multimodal,
        });
    }
    Ok(out)
}

/// Trae chat：POST {chat}/api/agent/v3/llm_utils_chat（自定义 SSE 事件流）
pub async fn trae_chat_stream(cred: &Cred, intl: bool, enterprise: bool, body: Value) -> Result<EventStream, String> {
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
    let resp = send_sse(req).await?;
    Ok(parse_trae_sse(resp.bytes_stream()))
}

// ============================ SSE 解析 ============================

/// 事件来源（决定缓冲与解析分支）
#[derive(Debug, Clone, Copy, PartialEq)]
enum SseKind {
    OpenAi,
    Trae,
}

/// SSE 泵：拉 chunk → 缓冲 → 逐条吐事件
struct Pump {
    kind: SseKind,
    buf: Vec<u8>,
    queue: std::collections::VecDeque<StreamEvent>,
    stream: std::pin::Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>,
    done: bool,
}

impl Pump {
    fn new(kind: SseKind, stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static) -> Self {
        Self { kind, buf: Vec::new(), queue: Default::default(), stream: Box::pin(stream), done: false }
    }
    fn feed(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }
    /// 队列优先，否则解析下一帧
    fn pop(&mut self) -> Option<StreamEvent> {
        if let Some(ev) = self.queue.pop_front() {
            return Some(ev);
        }
        loop {
            let (name, data) = self.next_block()?;
            match self.kind {
                SseKind::OpenAi => {
                    let evs: Vec<StreamEvent> = self.decode_openai(&data);
                    let mut it = evs.into_iter();
                    if let Some(first) = it.next() {
                        self.queue.extend(it);
                        return Some(first);
                    }
                }
                SseKind::Trae => {
                    if let Some(ev) = decode_trae_event(&name, &data) {
                        return Some(ev);
                    }
                }
            }
        }
    }
    /// 取下一个完整 SSE 块（空行分隔）；流已结束则处理残余
    fn next_block(&mut self) -> Option<(String, String)> {
        loop {
            if let Some(pos) = find_block_end(&self.buf) {
                let block = String::from_utf8_lossy(&self.buf[..pos.0]).to_string();
                self.buf.drain(..pos.1);
                if let Some(ev) = parse_event_block(&block) {
                    return Some(ev);
                }
                continue;
            }
            return None;
        }
    }
    fn decode_openai(&self, data: &str) -> Vec<StreamEvent> {
        if data == "[DONE]" {
            return vec![StreamEvent::Done { finish: "stop".into() }];
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else { return Vec::new() };
        parse_openai_chunk(&v)
    }
    /// 流结束后的残余帧
    fn take_tail(&mut self) -> Option<StreamEvent> {
        if let Some(ev) = self.queue.pop_front() {
            return Some(ev);
        }
        if self.buf.is_empty() {
            return None;
        }
        let block = String::from_utf8_lossy(&self.buf).to_string();
        self.buf.clear();
        let (name, data) = parse_event_block(&block)?;
        Some(match self.kind {
            SseKind::OpenAi => self.decode_openai(&data).into_iter().next()?,
            SseKind::Trae => decode_trae_event(&name, &data)?,
        })
    }

}

/// 块边界：\n\n 或 \r\n\r\n（返回 (内容长度, 总消耗)）
fn find_block_end(buf: &[u8]) -> Option<(usize, usize)> {
    for i in 0..buf.len().saturating_sub(1) {
        if buf[i] == b'\n' {
            if buf[i + 1] == b'\n' {
                return Some((i, i + 2));
            }
            if i + 3 < buf.len() && buf[i + 1] == b'\r' && buf[i + 2] == b'\n' {
                return Some((i, i + 4));
            }
            if buf[i - 1] == b'\r' && buf[i + 1] == b'\n' {
                // \r\n\n 混合：视为边界
                return Some((i - 1, i + 2));
            }
        }
    }
    None
}

fn parse_event_block(block: &str) -> Option<(String, String)> {
    let mut event_name = String::new();
    let mut data = String::new();
    for line in block.lines() {
        if let Some(e) = line.strip_prefix("event:") {
            event_name = e.trim().to_string();
        } else if let Some(d) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(d.strip_prefix(' ').unwrap_or(d));
        }
    }
    if data.is_empty() && event_name.is_empty() {
        return None;
    }
    Some((event_name, data))
}

/// OpenAI 线格式 chunk → 事件列表
pub fn parse_openai_chunk(v: &Value) -> Vec<StreamEvent> {
    let mut out = Vec::new();
    if let Some(err) = v.get("error") {
        let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("upstream error").to_string();
        let code = err.get("code").and_then(|c| {
            c.as_str().map(|s| s.to_string()).or_else(|| c.as_i64().map(|i| i.to_string()))
        }).unwrap_or_else(|| "upstream_error".into());
        return vec![StreamEvent::Error(msg, code)];
    }
    if let Some(code) = v.get("code").and_then(|c| c.as_i64()) {
        if code != 0 && v.get("choices").is_none() {
            let msg = v.get("message").or_else(|| v.get("msg")).and_then(|m| m.as_str()).unwrap_or("upstream refused").to_string();
            return vec![StreamEvent::Error(msg, code.to_string())];
        }
    }
    if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
        for ch in choices {
            let delta = ch.get("delta").cloned().unwrap_or(Value::Null);
            for key in ["reasoning_content", "reasoning"] {
                if let Some(rc) = delta.get(key).and_then(|r| r.as_str()) {
                    if !rc.is_empty() {
                        out.push(StreamEvent::Reasoning(rc.to_string()));
                    }
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
        if u.is_object() {
            let input = u.get("prompt_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
            let output = u.get("completion_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
            let total = u.get("total_tokens").and_then(|t| t.as_u64());
            if input > 0 || output > 0 {
                out.push(StreamEvent::Usage { input, output, total });
            }
        }
    }
    out
}

/// Trae 事件（decodeTraeEvent 对齐）→ StreamEvent
pub fn decode_trae_event(event_name: &str, data: &str) -> Option<StreamEvent> {
    if data == "[DONE]" {
        return Some(StreamEvent::Done { finish: "stop".into() });
    }
    let v: Value = serde_json::from_str(data).ok()?;
    match event_name {
        "request_wait_in_queue" => {
            let pos = v.get("position").and_then(|p| p.as_u64()).unwrap_or(0);
            Some(StreamEvent::Error(format!("上游排队中（第 {pos} 位），请稍后重试"), "queue".into()))
        }
        "progress_notice" | "metadata" | "timing_cost" => None,
        "token_usage" => Some(StreamEvent::Usage {
            input: v.get("prompt_tokens").and_then(|t| t.as_u64()).unwrap_or(0),
            output: v.get("completion_tokens").and_then(|t| t.as_u64()).unwrap_or(0),
            total: v.get("total_tokens").and_then(|t| t.as_u64()),
        }),
        "done" => Some(StreamEvent::Done {
            finish: v.get("finish_reason").and_then(|f| f.as_str()).unwrap_or("stop").to_string(),
        }),
        "error" => {
            let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
            let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string();
            let readable = match code {
                1005 => "该模型需要付费套餐".to_string(),
                4011 | 4001 => "该账号被限频或模型不在套餐内（可换账号/模型重试）".to_string(),
                _ => format!("上游错误 code={code}: {msg}"),
            };
            Some(StreamEvent::Error(readable, code.to_string()))
        }
        "output" => {
            let reasoning = v.get("reasoning_content").and_then(|r| r.as_str()).unwrap_or("");
            let text = v.get("response").and_then(|r| r.as_str()).unwrap_or("");
            // reasoning 优先（同一帧两者都有时先吐思考）
            if !reasoning.is_empty() {
                Some(StreamEvent::Reasoning(reasoning.to_string()))
            } else if !text.is_empty() {
                Some(StreamEvent::Text(text.to_string()))
            } else {
                None
            }
        }
        _ => {
            // 未命名事件兜底
            let reasoning = v.get("reasoning_content").and_then(|r| r.as_str()).unwrap_or("");
            let text = v.get("response").and_then(|r| r.as_str()).unwrap_or("");
            if !reasoning.is_empty() {
                Some(StreamEvent::Reasoning(reasoning.to_string()))
            } else if !text.is_empty() {
                Some(StreamEvent::Text(text.to_string()))
            } else if let Some(fr) = v.get("finish_reason").and_then(|f| f.as_str()) {
                Some(StreamEvent::Done { finish: fr.to_string() })
            } else {
                None
            }
        }
    }
}

fn pump_stream(kind: SseKind, stream: std::pin::Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>) -> EventStream {
    Box::pin(futures_util::stream::unfold(Pump::new(kind, stream), |mut p| async move {
        loop {
            if let Some(ev) = p.pop() {
                return Some((Ok(ev), p));
            }
            if p.done {
                return p.take_tail().map(|ev| (Ok(ev), p));
            }
            match p.stream.next().await {
                Some(Ok(chunk)) => p.feed(&chunk),
                Some(Err(e)) => return Some((Err(format!("stream: {e}")), p)),
                None => {
                    p.done = true;
                    if let Some(ev) = p.take_tail() {
                        return Some((Ok(ev), p));
                    }
                    return None;
                }
            }
        }
    }))
}

/// OpenAI 线格式 SSE → EventStream
pub fn parse_openai_sse(stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static) -> EventStream {
    pump_stream(SseKind::OpenAi, Box::pin(stream))
}

/// Trae 自定义 SSE → EventStream
pub fn parse_trae_sse(stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static) -> EventStream {
    pump_stream(SseKind::Trae, Box::pin(stream))
}
