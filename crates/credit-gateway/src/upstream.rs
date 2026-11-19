//! 上游调用层：模型目录抓取 + chat SSE 流式转发。
//!
//! chat 一律流式（WorkBuddy 上游拒绝非流式，Trae SOLO 通道也是 stream-only），
//! 非流式响应由网关层聚合。SSE 解析用统一的"泵"结构：拉 chunk → 行缓冲 → 逐条吐事件。

use std::time::Duration;

use futures_core::Stream;
use futures_util::StreamExt;
use serde_json::{json, Value};

use credit_core::adapters::Cred;
use credit_core::error::PlatformError;

use crate::protocol::*;

/// 上游 chat 产出的一条流事件
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// 正文文本增量
    Text(String),
    /// 思考/推理内容增量（reasoning_content）
    Reasoning(String),
    /// 工具调用增量（OpenAI `choices[].delta.tool_calls[]` 语义）
    ToolCall { index: u32, id: Option<String>, name: Option<String>, arguments: Option<String> },
    /// token 用量（input / output / total）
    Usage { input: u64, output: u64, total: Option<u64> },
    /// 流结束（finish_reason：stop / length / tool_calls …）
    Done { finish: String },
    /// 错误事件（消息, code）
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

/// 各平台统一的 chat 事件流类型（网关层消费）
pub type EventStream = std::pin::Pin<Box<dyn Stream<Item = Result<StreamEvent, String>> + Send>>;

/// 非流式请求客户端：整体超时 300s（普通 JSON 请求）
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

/// 流式（SSE）请求客户端：**不能用整体 timeout**——reqwest 的 timeout 覆盖整个响应体读取，
/// 长回答/长时间思考会被拦腰截断（客户端表现为「只收到一点数据就断开」）。
/// 这里只限制连接与「两次读取之间的空闲」，长流可以有任意总时长。
fn stream_client() -> &'static reqwest::Client {
    static C: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            // 深度思考模型可能连续数分钟不吐字节；空闲超时给足余量避免长任务被拦腰掐断
            .read_timeout(Duration::from_secs(300))
            .build()
            .expect("gateway stream client")
    })
}

/// 按字符截断到 n 个字符（超长补省略号；错误信息展示用）
fn truncate(s: &str, n: usize) -> String {
    // 按字符截断：按字节切片会在中文/emoji 边界 panic（panic 会中断整个响应）
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let t: String = s.chars().take(n).collect();
        format!("{t}…")
    }
}

/// 发送请求并校验 HTTP 2xx；非 2xx 读 body 截断后拼错误信息
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
/// 模型清单去重收敛：同名变体只留一个；剔除实验/内部通道；客户端三档（fast/balanced/deep）必留。
fn dedupe_models(mut models: Vec<ModelInfo>) -> Vec<ModelInfo> {
    // 实验通道特征：id 含这些片段的直接剔除（它们是同一底层模型的内部变体）
    let variant_markers = ["-ctrl", "-exp", "exp1", "exp2", "preview-x", "-volc", "-lkeap", "-taiji", "-gf"];
    models.retain(|m| !variant_markers.iter().any(|k| m.id.contains(k)));
    // 同名去重（name 相同视为同一模型，保留 id 最短的——通常是"主干"版本）
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut out: Vec<ModelInfo> = Vec::new();
    for m in models {
        let key = m.name.to_lowercase();
        match seen.get(&key) {
            Some(&idx) => {
                if m.id.len() < out[idx].id.len() {
                    out[idx] = m;
                }
            }
            None => {
                seen.insert(key, out.len());
                out.push(m);
            }
        }
    }
    // 客户端三档置顶（fast/balanced/deep-model 若存在则排最前）
    let priority = ["fast-model", "balanced-model", "deep-model"];
    out.sort_by_key(|m| {
        priority.iter().position(|p| m.id == *p).unwrap_or(priority.len())
    });
    out
}

/// WorkBuddy 模型目录：GET {chat_base}/v3/config（code!=0 信封）；
/// 只保留 clients=cli 桶内的可用模型（剔除 disabled/auto 与缺窗口的）
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
    // 去重收敛：剔除实验/内部通道变体，同名模型只留主干版本（三档置顶）
    out = dedupe_models(out);
    Ok(out)
}

/// WorkBuddy chat：POST {chat_base}/v2/chat/completions（OpenAI 线格式 SSE）
/// 上游额度/风控错误可能以 HTTP 200 + JSON 返回（如 14018 额度用尽），此处识别并透传。
pub async fn wb_chat_stream(cred: &Cred, body: Value, device: Option<(&str, &str, &str)>) -> Result<EventStream, String> {
    let domain = cred.meta.get("domain").and_then(|d| d.as_str()).unwrap_or("");
    let base = if wb_is_global(domain) { wb_chat_base(domain) } else { WB_CN_CHAT_BASE };
    let url = format!("{base}/v2/chat/completions");
    let enterprise = cred.meta.get("enterpriseId").and_then(|e| e.as_str());
    let hs = wb_chat_headers(&cred.token, &cred.uid, domain, enterprise, device);
    let mut req = stream_client().post(&url).json(&body);
    for (k, v) in &hs {
        req = req.header(k, v);
    }
    let resp = send_sse(req).await?;
    let is_sse = resp.headers().get("content-type").and_then(|v| v.to_str().ok())
        .map(|v| v.to_lowercase().contains("text/event-stream")).unwrap_or(false);
    if !is_sse {
        let text = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<Value>(&text).ok()
            .and_then(|v| {
                v.pointer("/error/data/msg").and_then(|m| m.as_str())
                    .or_else(|| v.pointer("/error/message").and_then(|m| m.as_str()))
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| text.chars().take(300).collect());
        let quota = text.contains("14018") || msg.contains("额度") || msg.contains("quota");
        return Err(format!("WorkBuddy 上游返回错误{}: {msg}", if quota { "（额度不足）" } else { "" }));
    }
    Ok(parse_openai_sse(resp.bytes_stream()))
}

// ============================ Trae ============================

/// 模型 → function 映射缓存（由 trae_fetch_models 从官方 models 目录填充）。
/// Trae 的模型只在列出它的 function 下可调用，chat 请求必须带正确的 function。
fn trae_function_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    static C: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> = std::sync::OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 查询模型所属 function（未抓过模型目录时返回 None）
pub fn trae_model_function(model: &str) -> Option<String> {
    trae_function_cache().lock().ok()?.get(model).cloned()
}

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
    // 兼容两种结构：旧 = data.list[] 直接模型行；新 = data.list[].function → models[]（按功能分组）
    let list = body.pointer("/data/list").and_then(|l| l.as_array()).cloned().unwrap_or_default();
    let mut rows: Vec<Value> = Vec::new();
    let mut grouped = false;
    // 模型 → 所属 function 映射：模型只在列出它的 function 下可调用（对话必须带对 function）
    let mut fn_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for entry in &list {
        if let Some(models) = entry.get("models").and_then(|m| m.as_array()) {
            grouped = true;
            let fname = entry.get("function").and_then(|f| f.as_str()).unwrap_or("").to_string();
            for m in models {
                let name = m.get("name").and_then(|n| n.as_str()).unwrap_or("");
                if !name.is_empty() && !fname.is_empty() {
                    fn_map.entry(name.to_string()).or_insert_with(|| fname.clone());
                }
            }
            rows.extend(models.iter().cloned());
        }
    }
    if !fn_map.is_empty() {
        if let Ok(mut cache) = trae_function_cache().lock() {
            for (k, v) in fn_map { cache.insert(k, v); }
        }
    }
    if !grouped {
        rows = list;
    }
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for row in rows {
        let id = row.get("name").and_then(|n| n.as_str()).unwrap_or_default().to_string();
        if id.is_empty() || !seen.insert(id.clone()) {
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
    // SOLO 通道需要 x-uid 头；meta 里没有时用凭据 uid 补上
    let mut meta = cred.meta.clone();
    if meta.get("uid").and_then(|v| v.as_str()).unwrap_or("").is_empty() {
        meta["uid"] = Value::String(cred.uid.clone());
    }
    let hs = trae_chat_headers(&cred.token, &meta);
    let mut req = stream_client().post(&url).json(&body);
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
    /// 已收到结束事件（finish_reason / [DONE]）：之后最多再等 2s 收尾（usage 等），
    /// 上游不关闭连接也不会让客户端一直挂着（否则客户端会超时断开，表现为「只收到一点数据」）
    finished: bool,
}

impl Pump {
    /// 构造泵：绑定解析分支与原始字节流
    fn new(kind: SseKind, stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static) -> Self {
        Self { kind, buf: Vec::new(), queue: Default::default(), stream: Box::pin(stream), done: false, finished: false }
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
    /// OpenAI data 帧 → 事件列表（[DONE] 视为 stop 结束）
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

/// 块边界：空行（\n\n / \r\n\r\n / \n\r\n / \r\n\n）。
/// 注意：必须做边界检查——旧实现 `buf[i-1]` 在 i==0 时越界 panic，
/// 而 panic 会直接杀死响应任务，客户端表现为「只收到一点数据就断开」。
fn find_block_end(buf: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0;
    while i < buf.len() {
        if buf[i] != b'\n' {
            i += 1;
            continue;
        }
        // \n\n
        if i + 1 < buf.len() && buf[i + 1] == b'\n' {
            return Some((i, i + 2));
        }
        // \n\r\n
        if i + 2 < buf.len() && buf[i + 1] == b'\r' && buf[i + 2] == b'\n' {
            return Some((i, i + 3));
        }
        // \r\n\n（前一个字节是 \r）
        if i > 0 && buf[i - 1] == b'\r' && i + 1 < buf.len() && buf[i + 1] == b'\n' {
            return Some((i - 1, i + 2));
        }
        i += 1;
    }
    None
}

/// 解析单个 SSE 块 → (event 名, data 文本)；两者皆空返回 None
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

/// 工具参数归一化为字符串：上游可能给 string（增量片段）或已解析的 object。
fn tool_args_str(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::Null) | None => None,
        Some(other) => {
            let s = other.to_string();
            if s.is_empty() { None } else { Some(s) }
        }
    }
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
            // 工具调用增量（Agent 客户端依赖）：delta.tool_calls[] 或兼容旧式 delta.function_call
            if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                for tc in tcs {
                    let index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
                    let id = tc.get("id").and_then(|i| i.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string());
                    let func = tc.get("function").cloned().unwrap_or(Value::Null);
                    let name = func.get("name").and_then(|n| n.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string());
                    let args = tool_args_str(func.get("arguments"));
                    if id.is_some() || name.is_some() || args.is_some() {
                        out.push(StreamEvent::ToolCall { index, id, name, arguments: args });
                    }
                }
            } else if let Some(func) = delta.get("function_call") {
                let name = func.get("name").and_then(|n| n.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string());
                let args = tool_args_str(func.get("arguments"));
                if name.is_some() || args.is_some() {
                    out.push(StreamEvent::ToolCall { index: 0, id: None, name, arguments: args });
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
            } else { v.get("finish_reason").and_then(|f| f.as_str()).map(|fr| StreamEvent::Done { finish: fr.to_string() }) }
        }
    }
}

/// 用泵驱动原始字节流 → 统一 EventStream（Done 后最多再等 2s 收尾）
fn pump_stream(kind: SseKind, stream: std::pin::Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>) -> EventStream {
    Box::pin(futures_util::stream::unfold(Pump::new(kind, stream), |mut p| async move {
        loop {
            if let Some(ev) = p.pop() {
                if matches!(ev, StreamEvent::Done { .. }) {
                    p.finished = true;
                }
                return Some((Ok(ev), p));
            }
            if p.done {
                return p.take_tail().map(|ev| (Ok(ev), p));
            }
            // 已收到结束事件：最多再等 2s 收尾（usage / 剩余帧），避免上游不关闭连接时客户端挂死
            let next = if p.finished {
                match tokio::time::timeout(Duration::from_millis(2000), p.stream.next()).await {
                    Ok(v) => v,
                    Err(_) => return p.take_tail().map(|ev| (Ok(ev), p)),
                }
            } else {
                p.stream.next().await
            };
            match next {
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


// ============================ ZCode（智谱） ============================

/// 智谱模型清单：对齐官方 ZCode 客户端 builtin 目录（3.18.2，resources/glm/provider/
/// zcode-builtin.json 逆向）——Start Plan（account:zai/bigmodel-start-plan）builtin 声明
/// GLM-5.3-Flash / GLM-5.2 / GLM-5-Turbo；GLM-5.3 属 Coding Plan 档。
/// 旧清单（glm-5.3-flash 小写 / glm-5-flash / glm-5.3-air / glm-4.7）是错的，上游不认识。
///
/// 2026-11-19 Start Plan 端点实测：GLM-5.3-Flash / GLM-5.3 返回 200；GLM-5.2 与
/// GLM-5-Turbo 返回 400 code=3006（model not allowed）—— 按账号权益放开，故保留在清单里
/// 但请求时会给出明确报错。
pub async fn zcode_fetch_models(_cred: &Cred) -> Result<Vec<ModelInfo>, PlatformError> {
    let mut out = Vec::new();
    for (id, name, ctx) in [
        ("GLM-5.3-Flash", "GLM-5.3-Flash", 200_000u64),
        ("GLM-5.2", "GLM-5.2", 128_000),
        ("GLM-5-Turbo", "GLM-5-Turbo", 128_000),
        // Coding Plan 档模型（账号带 coding plan 权益时可用）
        ("GLM-5.3", "GLM-5.3", 200_000),
    ] {
        out.push(ModelInfo { id: id.into(), name: name.into(), context_window: Some(ctx), max_tokens: None, credit_multiplier: None, supports_images: None });
    }
    Ok(out)
}

/// ZCode chat 通道（2026-11-19 实测结论：Start Plan 专属端点可用了）
///
/// **① 首选：Start Plan 专属端点**（官方客户端同款）
///    `POST https://zcode.z.ai/api/v1/zcode-plan/anthropic/v1/messages`
///    凭据 = 本机 ZCode 的 `zcodejwttoken`（长效会话 JWT，无 exp）。
///
///    该端点有服务端「仅官方客户端」闸门（`code=3012` / HTTP 405）。2026-11-19 逐项消融后
///    确认闸门 = **两项同时满足**：
///    ① 请求头 `X-ZCode-App-Version: 3.18.2`（当前客户端版本）；
///    ② 请求体 `system` 的前两块是官方 CLI 固定文本块（CLI Prefix + Identity/Harness），
///       顺序不可颠倒。
///    官方 CLI 自己发的请求头是 `0.17.1`（AI SDK 默认值），所以官方 CLI 也会被 3012 拒；
///    把该头改成 3.18.2 后，同一请求体立刻 200 并正常流式返回。
///
/// **② 回退：bigmodel Coding-Plan Anthropic 端点**
///    `POST https://open.bigmodel.cn/api/anthropic/v1/messages`
///    凭据 = 用长效 zcodejwttoken 经 `GET /api/v1/oauth/profile` 动态换取的新鲜
///    bigmodel access_token（官方客户端的取票通道，实测 code=0）。
///    认证有效（429=额度不足是上游业务错误，非鉴权失败）。
pub async fn zcode_chat_stream(cred: &Cred, body: Value) -> Result<EventStream, String> {
    let mid = cred.meta.get("deviceMid").and_then(|d| d.as_str()).unwrap_or("");
    let model = body.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
    let max_tokens = body.get("max_tokens").and_then(|v| v.as_u64()).map(|v| v as u32);
    // OpenAI messages → Anthropic 线格式（system 顶层 + tool_use/tool_result 转换）
    let msgs: Vec<Value> = body.get("messages").and_then(|m| m.as_array()).cloned().unwrap_or_default();
    let system = body.get("system").and_then(|s| s.as_str()).unwrap_or("").to_string();
    let (a_msgs, sys) = crate::protocol::to_anthropic_messages(&msgs, &system);
    let mut a_body = crate::protocol::zcode_anthropic_body(&a_msgs, &sys, &model, max_tokens);
    // 工具（Anthropic 风格）与 thinking 开关
    let tools: Vec<Value> = body.get("tools").and_then(|t| t.as_array()).cloned().unwrap_or_default();
    let tool_choice = body.get("tool_choice").cloned();
    crate::protocol::attach_anthropic_tools(&mut a_body, &tools, tool_choice.as_ref());

    let jwt = cred.meta.get("zcodeJwt").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if jwt.is_empty() {
        return Err("本机 ZCode 凭据缺少 zcodejwttoken（对话取票令牌），请重新登录 ZCode 客户端并「扫描本机」".into());
    }

    // —— ① Start Plan 专属端点（官方同款）——
    // system 前两块必须是官方固定文本，否则服务端返回 3012。
    a_body["system"] = Value::Array(crate::protocol::zcode_start_plan_system_blocks(&sys));
    a_body["stream"] = Value::Bool(true);

    let url = format!("{}{}", crate::protocol::ZCODE_PLAN_ANTHROPIC_BASE, crate::protocol::ZCODE_ANTHROPIC_PATH);
    let hs = crate::protocol::zcode_plan_chat_headers(&jwt, mid);
    let mut req = stream_client().post(&url).json(&a_body);
    for (k, v) in &hs { req = req.header(k, v); }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => return Err(format!("ZCode 对话端点请求失败（{url}）：{e}")),
    };
    let st = resp.status().as_u16();
    if st == 200 {
        return Ok(parse_anthropic_sse(resp.bytes_stream()));
    }
    let txt = resp.text().await.unwrap_or_default();
    // 3012 = 官方客户端身份闸门未过（版本头或 system 块不匹配）
    if txt.contains("3012") {
        return Err(format!(
            "ZCode Start Plan 被服务端「仅官方客户端」校验拒绝（code=3012）：{txt}；\
             请确认已升级到 3.18.2 客户端（X-ZCode-App-Version 需与当前版本一致）"
        ));
    }
    // 3006 = 该账号无权使用此模型（官方 builtin 目录列出 3 个模型，但按账号权益放开）
    if txt.contains("3006") {
        return Err(format!(
            "ZCode Start Plan 当前账号无权使用该模型（code=3006）：{txt}；\
             当前账号实测可用 GLM-5.3-Flash / GLM-5.3（GLM-5.2、GLM-5-Turbo 需更高档权益）"
        ));
    }
    // 3007 = 安全校验（验证码）拦截：官方客户端会弹验证码，第三方无法自动过
    if txt.contains("3007") {
        return Err(format!(
            "ZCode Start Plan 触发安全校验（code=3007）：{txt}；该档位需要官方客户端完成验证码，\
             请改用其它平台或等待风控解除"
        ));
    }
    // 1113 = 额度不足（账号无可用资源包）
    if txt.contains("1113") {
        return Err(format!(
            "ZCode 对话被拒（HTTP {st} code=1113 余额不足或无可用资源包）：{txt}"
        ));
    }
    if txt.contains("401") || st == 401 {
        return Err(format!(
            "ZCode 令牌被拒（HTTP {st}）：zcodejwttoken 可能已失效，请在 ZCode 客户端重新登录后「扫描本机」"
        ));
    }

    // —— ② 回退：bigmodel Coding-Plan 端点 ——
    // 该端点没有 3012 闸门，用用户自己的 system 提示词即可（不必带官方固定块）。
    if !sys.is_empty() {
        a_body["system"] = json!([{ "type": "text", "text": sys }]);
    } else {
        a_body.as_object_mut().map(|o| o.remove("system"));
    }
    let fresh = match credit_core::adapters::zcode::fresh_bigmodel_token(cred).await {
        Ok(t) => t,
        Err(e) => return Err(format!("ZCode Start Plan 请求失败且取票回退也失败：{e}")),
    };
    let url = format!("{}{}", crate::protocol::ZCODE_ANTHROPIC_BASE, crate::protocol::ZCODE_ANTHROPIC_PATH);
    let mut hs = crate::protocol::zcode_chat_headers(&fresh, Some(mid));
    hs.push(("anthropic-version".to_string(), "2023-06-01".to_string()));
    let mut req = stream_client().post(&url).json(&a_body);
    for (k, v) in &hs { req = req.header(k, v); }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => return Err(format!("ZCode 对话端点请求失败（{url}）：{e}")),
    };
    let st = resp.status().as_u16();
    if st == 200 {
        return Ok(parse_anthropic_sse(resp.bytes_stream()));
    }
    let txt = resp.text().await.unwrap_or_default();
    if txt.contains("1113") {
        return Err(format!(
            "ZCode 对话被拒（HTTP {st} code=1113 余额不足或无可用资源包）：当前账号在 bigmodel 平台没有可用的 \
             Coding Plan 权益（Start Plan 权益只挂在官方客户端专属端点上）。请确认 ZCode 客户端里已激活套餐，\
             或改用其它平台的模型"
        ));
    }
    if txt.contains("401") || st == 401 {
        return Err(format!(
            "ZCode 取票 token 被拒（HTTP {st}）：zcodejwttoken 可能已失效，请在 ZCode 客户端重新登录后「扫描本机」"
        ));
    }
    Err(format!("ZCode 对话端点返回 HTTP {st}：{}", truncate(&txt, 240)))
}

// ============================ MiniMax ============================

/// MiniMax 模型清单：anthropic 兼容通道无 models 端点 → 静态已知清单。
/// OAuth token 会话验证 profile 已由 credit-core 的 verify 处理；此处直接给静态目录。
pub async fn minimax_fetch_models(_cred: &Cred) -> Result<Vec<ModelInfo>, PlatformError> {
    // 对齐官方客户端 first-run 目录（asar 逆向 minimax-model-catalog.js；M2.x 已 retired）
    Ok([
        ("MiniMax-M3.1-Flash", "M3.1-Flash", 512_000u64),
        ("MiniMax-M3", "MiniMax-M3", 512_000),
        // 兼容历史：部分账号仍可调 M2.5/M2.7
        ("MiniMax-M2.5", "MiniMax-M2.5", 200_000),
        ("MiniMax-M2.7", "MiniMax-M2.7", 200_000),
    ].into_iter().map(|(id, name, ctx)| ModelInfo {
        id: id.into(), name: name.into(), context_window: Some(ctx),
        max_tokens: None, credit_multiplier: None, supports_images: None,
    }).collect())
}

/// MiniMax chat：POST api.minimaxi.com/anthropic/v1/messages（Anthropic 线格式 SSE）
/// 凭据：OAuth mmoat token（部分会话可用）或 MiniMax API key。401 时报明确错误。
pub async fn minimax_chat_stream(cred: &Cred, body: Value) -> Result<EventStream, String> {
    let url = format!("{MINIMAX_API_BASE}{MINIMAX_ANTHROPIC_PATH}");
    let hs = minimax_chat_headers(&cred.token);
    let mut req = stream_client().post(&url).json(&body);
    for (k, v) in &hs {
        req = req.header(k, v);
    }
    let resp = send_sse(req).await?;
    Ok(parse_anthropic_sse(resp.bytes_stream()))
}

/// Anthropic 线格式 SSE → EventStream（复用 OpenAI 帧格式；事件解码走 anthropic 分支）
pub fn parse_anthropic_sse(stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static) -> EventStream {
    Box::pin(futures_util::stream::unfold(AnthropicPump::new(stream), |mut p| async move {
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

/// Anthropic SSE 泵（帧格式与 OpenAI 相同：data: 行）。
/// 持有「content block index → tool_calls index」映射：工具参数分片按块归组，
/// 客户端才能把 input_json_delta 拼回对应 tool call（否则工具调用参数丢失）。
struct AnthropicPump {
    buf: Vec<u8>,
    queue: std::collections::VecDeque<StreamEvent>,
    stream: std::pin::Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>,
    done: bool,
    /// 已发出结束事件（message_delta stop_reason / message_stop）：之后不再重复 Done
    finished: bool,
    block_tool_index: std::collections::HashMap<u64, u32>,
    next_tool_index: u32,
}

impl AnthropicPump {
    fn new(stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static) -> Self {
        Self {
            buf: Vec::new(),
            queue: Default::default(),
            stream: Box::pin(stream),
            done: false,
            finished: false,
            block_tool_index: Default::default(),
            next_tool_index: 0,
        }
    }
    fn feed(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }
    fn pop(&mut self) -> Option<StreamEvent> {
        if let Some(ev) = self.queue.pop_front() {
            return Some(ev);
        }
        loop {
            let block = self.next_block()?;
            let mut evs: Vec<StreamEvent> = Vec::new();
            for line in block.lines() {
                if let Some(data) = line.strip_prefix("data:") {
                    let d = data.trim();
                    if d.is_empty() { continue; }
                    evs.extend(self.decode(d));
                }
            }
            let mut it = evs.into_iter();
            if let Some(first) = it.next() {
                self.queue.extend(it);
                return Some(first);
            }
        }
    }
    fn next_block(&mut self) -> Option<String> {
        loop {
            let pos = find_block_end(&self.buf)?;
            let block = String::from_utf8_lossy(&self.buf[..pos.0]).to_string();
            self.buf.drain(..pos.1);
            if block.trim().is_empty() { continue; }
            return Some(block);
        }
    }
    /// data 帧解码：统一走 protocol 的映射解析；Done 只发一次（message_delta 与 message_stop 常同时出现）
    fn decode(&mut self, data: &str) -> Vec<StreamEvent> {
        if data == "[DONE]" {
            if self.finished { return Vec::new(); }
            self.finished = true;
            return vec![StreamEvent::Done { finish: "stop".into() }];
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else { return Vec::new() };
        let evs = crate::protocol::anthropic_chunk_with_map(&v, &mut self.block_tool_index, &mut self.next_tool_index);
        let mut out = Vec::new();
        for ev in evs {
            if matches!(ev, StreamEvent::Done { .. }) {
                if self.finished { continue; }
                self.finished = true;
            }
            out.push(ev);
        }
        out
    }
    fn take_tail(&mut self) -> Option<StreamEvent> {
        if let Some(ev) = self.queue.pop_front() {
            return Some(ev);
        }
        if self.buf.is_empty() { return None; }
        let block = String::from_utf8_lossy(&self.buf).to_string();
        self.buf.clear();
        for line in block.lines() {
            if let Some(data) = line.strip_prefix("data:") {
                let d = data.trim();
                if !d.is_empty() {
                    if let Some(ev) = self.decode(d).into_iter().next() {
                        return Some(ev);
                    }
                }
            }
        }
        None
    }
}


// ============================ Qoder ============================

/// Qoder 模型清单：云端 chat 的模型是网关自动路由（"auto"），
/// 公开目录无 models 端点 → 静态发布网关模型。
///
/// 2026-11-19 逆向校准（Qoder CN 0.5.14）：
/// - 权威目录来自客户端 worker `models.list`（GET gateway.qoder.com.cn/algo/api/v2/model/list，
///   需 Cosy 签名）live 拉取，共 14 个；完整快照见 `docs/qoder-catalog-live-2026-11-19.json`。
/// - `efficient` / `performance` / `ultimate` 已被官方删除（上游实测 invalid_model_error），
///   旧清单里的 `lite`/`cmodel`/`gm51model`/`q37fmodel`/`qmodel` 也已下架（部分仍可路由，不再对外发布）。
/// - 新增 `jmodel`(Step-5-Preview) / `qmodel_latest_preview`(Qwen4-Max-Preview)；
///   `qmodel_latest` 线上显示名已变为 Qwen4-Max。
pub async fn qoder_fetch_models(_cred: &Cred) -> Result<Vec<ModelInfo>, PlatformError> {
    Ok([
        ("auto", "Auto", 200_000u64),
        ("fast", "Fast", 200_000),
        ("qmodel_latest", "Qwen4-Max", 1_000_000),
        ("qmodel_latest_preview", "Qwen4-Max-Preview", 1_000_000),
        ("qmodel_38max", "Qwen3.8-Max", 180_000),
        ("qfmodel", "Qwen3.8-Flash", 180_000),
        ("dmodel", "DeepSeek-V4-Pro", 96_000),
        ("dfmodel", "DeepSeek-Flash", 180_000),
        ("gmodel", "GLM-5.3", 180_000),
        ("gfmodel", "GLM-5.3-Flash", 1_000_000),
        ("kmodel_latest", "Kimi-K3.1-Pro", 180_000),
        ("kmodel", "Kimi-K2.8-Preview", 180_000),
        ("jmodel", "Step-5-Preview", 1_000_000),
        ("mmodel", "MiniMax-M3.1-Flash", 180_000),
    ].into_iter().map(|(id, name, ctx)| ModelInfo {
        id: id.into(), name: name.into(), context_window: Some(ctx),
        max_tokens: None, credit_multiplier: None,
        // 当前目录 14 个模型全部 isVl=true（原生多模态）
        supports_images: Some(true),
    }).collect())
}

/// Qoder 签名 sidecar 常驻进程（每进程一个，stdio 协议）
pub struct QoderSigner {
    #[allow(dead_code)] // 保留句柄防止子进程被 drop 关闭；stderr 由系统接管
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
}

impl QoderSigner {
    /// 启动 sidecar 子进程并等待 ready 帧（stdio 管道接管）
    pub async fn spawn() -> Result<Self, PlatformError> {
        let exe = sidecar_script()?;
        let node_bin = find_node()?;
        let mut cmd = tokio::process::Command::new(&node_bin);
        cmd.arg(&exe)
            .arg("serve")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        // Windows：node 是控制台子系统进程，GUI 应用里 spawn 会弹黑框 → 隐藏后台运行
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000);
        let child = cmd
            .spawn()
            .map_err(|e| PlatformError::Local(format!("sidecar spawn: {e}")))?;
        let mut c = child;
        let stdin = c.stdin.take().ok_or_else(|| PlatformError::Local("sidecar stdin".into()))?;
        let mut stdout = c.stdout.take().ok_or_else(|| PlatformError::Local("sidecar stdout".into()))?;
        // 消费 ready 帧 {"ok":true,"ready":true}
        {
            use tokio::io::AsyncBufReadExt;
            let mut reader = tokio::io::BufReader::new(&mut stdout);
            let mut ready_line = String::new();
            reader.read_line(&mut ready_line).await
                .map_err(|e| PlatformError::Local(format!("sidecar ready: {e}")))?;
        }
        let stdout = stdout;
        Ok(Self { child: c, stdin, stdout })
    }

    /// 请求签名：发 JSON 行 → 收 JSON 行
    pub async fn sign(&mut self, req: &Value) -> Result<Value, PlatformError> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let line = serde_json::to_string(req).map_err(|e| PlatformError::Local(e.to_string()))?;
        self.stdin.write_all(line.as_bytes()).await
            .map_err(|e| PlatformError::Local(format!("sidecar write: {e}")))?;
        self.stdin.write_all(b"\n").await
            .map_err(|e| PlatformError::Local(format!("sidecar write: {e}")))?;
        self.stdin.flush().await
            .map_err(|e| PlatformError::Local(format!("sidecar write: {e}")))?;
        let mut reader = tokio::io::BufReader::new(&mut self.stdout);
        let mut resp_line = String::new();
        reader.read_line(&mut resp_line).await
            .map_err(|e| PlatformError::Local(format!("sidecar read: {e}")))?;
        let resp: Value = serde_json::from_str(resp_line.trim())
            .map_err(|e| PlatformError::Local(format!("sidecar json: {e}")))?;
        if resp.get("ok").and_then(|o| o.as_bool()).unwrap_or(false) {
            Ok(resp)
        } else {
            Err(PlatformError::Local(format!("sidecar: {}", resp.get("error").and_then(|e| e.as_str()).unwrap_or("unknown"))))
        }
    }
}

/// 探测可用的 node 可执行文件（跨平台：node / nodejs）
fn find_node() -> Result<String, PlatformError> {
    if let Ok(p) = std::env::var("NODE_BIN") { return Ok(p); }
    for name in ["node", "nodejs"] {
        let mut cmd = std::process::Command::new(name);
        cmd.arg("--version");
        if let Ok(out) = credit_core::proc_util::hide_console(cmd).output() {
            if out.status.success() { return Ok(name.to_string()); }
        }
    }
    // GUI 进程 PATH 可能缺 node —— 探测常见安装位置
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let mut candidates = vec![
        "C:/Program Files/nodejs/node.exe".to_string(),
        "D:/ProgramFiles/develop/nodejs/node.exe".to_string(),
        format!("{local}/Programs/nodejs/node.exe"),
        "C:/nvm4w/nodejs/node.exe".to_string(),
        "D:/ProgramFiles/develop/nvm/nodejs/node.exe".to_string(),
    ];
    // nvm 管理的版本目录（nodejs symlink 常指向它）
    if let Ok(entries) = std::fs::read_dir("D:/ProgramFiles/develop/nvm") {
        for e in entries.filter_map(|e| e.ok()) {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false)
                && e.file_name().to_string_lossy().starts_with('v') {
                    candidates.push(format!("D:/ProgramFiles/develop/nvm/{}/node.exe", e.file_name().to_string_lossy()));
                }
        }
    }
    for c in candidates {
        if std::path::Path::new(&c).exists() {
            let mut cmd = std::process::Command::new(&c);
            cmd.arg("--version");
            if let Ok(out) = credit_core::proc_util::hide_console(cmd).output() {
                if out.status.success() { return Ok(c); }
            }
        }
    }
    Err(PlatformError::Local("node.js not found; install Node.js or set NODE_BIN".into()))
}

/// 编译期嵌入 Qoder 签名器（开发运行 / 未打包资源时也能用；无需依赖 Node 之外的运行时文件布局）
const QODER_SIGNER_JS: &str = include_str!("../sidecar/qoder-signer.mjs");
/// 编译期嵌入 Qoder 签名 WASM（与 js 配套释放到临时目录）
const QODER_SIGNER_WASM: &[u8] = include_bytes!("../sidecar/qoder_auth_wasm_bg.wasm");

/// 定位 qoder-signer.mjs：环境变量 → 安装/资源目录 → 仓库布局 → 临时目录释放内嵌副本
fn sidecar_script() -> Result<std::path::PathBuf, PlatformError> {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("QODER_SIGNER") {
        candidates.push(std::path::PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe() {
        let dir = exe.parent().map(|p| p.to_path_buf());
        if let Some(d) = &dir {
            // ① 打包后 exe 同级 sidecar/；② Tauri 资源目录；③ 数组式 resources 的 _up_ 布局
            for rel in [
                "sidecar/qoder-signer.mjs",
                "resources/sidecar/qoder-signer.mjs",
                "_up_/crates/credit-gateway/sidecar/qoder-signer.mjs",
                "_up_/sidecar/qoder-signer.mjs",
            ] {
                candidates.push(d.join(rel));
            }
        }
        // ④ 从 exe 所在目录逐级向上找仓库布局（dev: target/debug → 项目根）
        let mut cur = dir;
        for _ in 0..6 {
            let Some(d) = cur else { break };
            candidates.push(d.join("crates/credit-gateway/sidecar/qoder-signer.mjs"));
            candidates.push(d.join("sidecar/qoder-signer.mjs"));
            cur = d.parent().map(|p| p.to_path_buf());
        }
    }
    // ⑤ 当前工作目录（本地启动项目时通常是仓库根）
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("crates/credit-gateway/sidecar/qoder-signer.mjs"));
        candidates.push(cwd.join("sidecar/qoder-signer.mjs"));
    }
    // ⑥ 编译期源码目录（本机编译出的程序直接命中）
    candidates.push(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("sidecar/qoder-signer.mjs"));

    for c in candidates {
        if c.exists() {
            return Ok(c);
        }
    }

    // ⑦ 兜底：把内嵌的 js+wasm 释放到临时目录（安装到其它机器、资源缺失时仍可用）
    let dir = std::env::temp_dir().join("credit-rs-sidecar");
    std::fs::create_dir_all(&dir)
        .map_err(|e| PlatformError::Local(format!("sidecar dir: {e}")))?;
    let js = dir.join("qoder-signer.mjs");
    let wasm = dir.join("qoder_auth_wasm_bg.wasm");
    let js_stale = std::fs::read(&js).map(|b| b != QODER_SIGNER_JS.as_bytes()).unwrap_or(true);
    if js_stale {
        std::fs::write(&js, QODER_SIGNER_JS)
            .map_err(|e| PlatformError::Local(format!("sidecar extract: {e}")))?;
    }
    let wasm_stale = std::fs::read(&wasm).map(|b| b != QODER_SIGNER_WASM).unwrap_or(true);
    if wasm_stale {
        std::fs::write(&wasm, QODER_SIGNER_WASM)
            .map_err(|e| PlatformError::Local(format!("sidecar extract: {e}")))?;
    }
    Ok(js)
}

/// Qoder 签名 sidecar 进程池槽数：单进程签名是串行的（毫秒级），多槽位轮转支撑本地网关并发。
const QODER_SIGNER_POOL: usize = 4;

/// 签名进程池槽位（惰性初始化；槽内 None = 尚未 spawn）
fn signer_slots() -> &'static [std::sync::Arc<tokio::sync::Mutex<Option<QoderSigner>>>] {
    static SLOTS: std::sync::OnceLock<Vec<std::sync::Arc<tokio::sync::Mutex<Option<QoderSigner>>>>> = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| {
        (0..QODER_SIGNER_POOL)
            .map(|_| std::sync::Arc::new(tokio::sync::Mutex::new(None)))
            .collect()
    })
}

/// 从进程池轮转取一个常驻签名进程完成签名（旧实现每请求 spawn 一个 node 进程，
/// 高并发下进程创建开销大且频繁弹进程）；进程死亡时原地重建后重试一次。
async fn qoder_sign(req: &Value) -> Result<Value, String> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let slots = signer_slots();
    let idx = NEXT.fetch_add(1, Ordering::Relaxed) % slots.len();
    let slot = slots[idx].clone();
    let mut guard = slot.lock().await;
    if guard.is_none() {
        *guard = Some(QoderSigner::spawn().await.map_err(|e| e.to_string())?);
    }
    let signer = guard.as_mut().expect("signer slot must be filled");
    match signer.sign(req).await {
        Ok(v) => Ok(v),
        Err(e) => match QoderSigner::spawn().await {
            Ok(mut fresh) => {
                let out = fresh.sign(req).await.map_err(|e| e.to_string());
                *guard = Some(fresh);
                out
            }
            Err(_) => {
                *guard = None;
                Err(e.to_string())
            }
        },
    }
}

/// 由账号 uid 派生稳定的会话 UUID（同一账号所有请求复用一个上游会话）。
/// 码道上游限制「并发会话数 ≤ 3」：早期实现每请求生成新 user-session-id，
/// Agent 并发/多轮调用时瞬间创建多个会话，触发 TM.00001041 直接失败（表现为请求断掉/无法处理）。
fn stable_session_id(uid: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"credit-rs/session/");
    h.update(uid.as_bytes());
    let d = h.finalize();
    let hex = d.iter().map(|b| format!("{b:02x}")).collect::<String>();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

/// 华为码道 chat：POST {snap-access}/api/v2/chat/completions（OpenAI 兼容 SSE）。
/// 签名头集与 PromptCenter 门面不同：无 agent-type；x-auth-token 与 x-security-token 同值。
/// desktop/IDE 仅影响通道头（x-ot-client-type / ide-name / plugin-name）。
pub async fn codearts_chat_stream(cred: &Cred, body: Value, desktop: bool) -> Result<EventStream, String> {
    let sts = cred.meta.get("security_token").and_then(|t| t.as_str()).unwrap_or("");
    let ak = cred.meta.get("access_key_id").and_then(|a| a.as_str()).unwrap_or("");
    let sk = cred.meta.get("secret_access_key").and_then(|a| a.as_str()).unwrap_or("");
    if ak.is_empty() || sk.is_empty() {
        return Err("码道凭据缺少 AK/SK —— 请重新扫描导入".into());
    }
    let host = "snap-access.cn-north-4.myhuaweicloud.com";
    let path = "/api/v2/chat/completions";
    let body_str = serde_json::to_string(&body).map_err(|e| e.to_string())?;
    let model = body.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();

    // 通道头（桌面 vs IDE 插件；凭据共用同一 STS 会话）
    let client_type = if desktop { "desktop" } else { "ide" };
    let ide_name = if desktop { "codearts_space" } else { "codemate_vscode" };
    let trace_hex: String = {
        let mut b = [0u8; 16];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut b);
        b.iter().map(|x| format!("{x:02x}")).collect()
    };
    let span_hex: String = {
        let mut b = [0u8; 8];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut b);
        b.iter().map(|x| format!("{x:02x}")).collect()
    };
    // 实测：网关校验 x-span-id 类头 max length 64 —— x-snap-traceid 不能拼 uuid_hex(65)，
    // 用单段 hex32；span/trace 各自独立 hex 亦在 64 内
    let trace = trace_hex.clone();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
    let (y, mo, d) = credit_core::adapters::civil_from_days(now / 86400);
    let rem = now % 86400;
    let created = format!("{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, (rem % 3600) / 60, rem % 60);

    // 业务头（content-type/host/x-sdk-date 由 sign_codearts 内部补全并纳入签名）
    let extra: Vec<(String, String)> = vec![
        ("x-security-token".into(), sts.to_string()),
        ("x-auth-token".into(), sts.to_string()),
        ("user-session-id".into(), stable_session_id(&cred.uid)),
        ("x-snap-traceid".into(), trace.clone()),
        ("x-span-id".into(), uuid::Uuid::new_v4().to_string()),
        ("traceid".into(), trace.clone()),
        ("x-ot-trace-id".into(), trace.clone()),
        ("x-ot-span-id".into(), span_hex),
        ("x-ot-client-version".into(), "0.2.9".into()),
        ("x-ot-client-type".into(), client_type.into()),
        ("ide-name".into(), ide_name.into()),
        ("ide-version".into(), "0.2.9".into()),
        ("plugin-name".into(), ide_name.into()),
        ("plugin-version".into(), "0.2.9".into()),
        ("user-msg-id".into(), format!("msg_{}", uuid::Uuid::new_v4().simple())),
        ("created-time".into(), created),
        ("model-id".into(), model.clone()),
        ("model-name".into(), model),
    ];
    let host_of = host.to_string();
    // 共享流式 client（连接复用；旧实现每请求新建 Client 各自维护连接池，浪费且慢）
    let client = stream_client();
    let url = format!("https://{host}{path}");
    // 并发会话上限（TM.00001041）自动退避重试：Agent 并发调用时偶发，等前序会话结束后即可成功
    let mut last_err = String::new();
    for attempt in 0..4u32 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64 + 300)).await;
        }
        let (auth, signed) = credit_core::adapters::codearts::sign_codearts(ak, sk, "POST", &host_of, path, "", &extra, &body_str, Some(sts));
        let mut req = client.post(&url).header("Content-Type", "application/json");
        for (k, v) in &signed {
            req = req.header(k, v);
        }
        req = req.header("Authorization", &auth).body(body_str.clone());
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => return Err(format!("network: {e}")),
        };
        let status = resp.status().as_u16();
        if (200..300).contains(&status) {
            return Ok(parse_openai_sse(resp.bytes_stream()));
        }
        let text = resp.text().await.unwrap_or_default();
        let busy = text.contains("TM.00001041") || text.contains("并发会话数");
        last_err = format!("CodeArts chat HTTP {status}: {}", truncate(&text, 300));
        if !busy {
            return Err(last_err);
        }
        eprintln!("[codearts] 并发会话已满，退避重试 {}/4", attempt + 1);
    }
    Err(last_err)
}

/// 华为码道模型清单：GET {API_BASE}/v1/model/builtin（HWS 签名 + x-security-token）
pub async fn codearts_fetch_models(cred: &Cred, _desktop: bool) -> Result<Vec<ModelInfo>, PlatformError> {
    // 官方客户端 listBuiltinModels 形态：GET /v1/model/builtin + X-Security-Token + Agent-Type: PromptCenter
    let sts = cred.meta.get("security_token").and_then(|t| t.as_str()).unwrap_or("");
    let ak = cred.meta.get("access_key_id").and_then(|a| a.as_str()).unwrap_or("");
    let sk = cred.meta.get("secret_access_key").and_then(|a| a.as_str()).unwrap_or("");
    let base = "https://snap-access.cn-north-4.myhuaweicloud.com";
    let url = format!("{base}/v1/model/builtin");
    // header 名统一小写，与 sign_codearts 的 SignedHeaders 一致（HWS 按字节匹配）
    let extra_headers: Vec<(String, String)> = vec![
        ("x-security-token".into(), sts.to_string()),
        ("agent-type".into(), "PromptCenter".into()),
        ("x-language".into(), "zh-cn".into()),
    ];
    let body_str = "";
    // 用 credit-core 的 sign_codearts 构造 HWS 签名头
    let host = "snap-access.cn-north-4.myhuaweicloud.com";
    let (auth, signed) = credit_core::adapters::codearts::sign_codearts(ak, sk, "GET", host, "/v1/model/builtin", "", &extra_headers, body_str, Some(sts));
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build().unwrap();
    let mut req = client.get(&url);
    for (k, v) in &signed { req = req.header(k, v); }
    req = req.header("Authorization", &auth);
    let resp = req.send().await.map_err(|e| PlatformError::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    let v: Value = resp.json().await.map_err(|e| PlatformError::Format(e.to_string()))?;
    if status != 200 {
        return Err(PlatformError::Format(format!("builtin HTTP {status}: {}", serde_json::to_string(&v).unwrap_or_default().chars().take(200).collect::<String>())));
    }
    let mut out = Vec::new();
    // 官方响应：{"builtinModels": [{model_id, model_name, context_window, max_tokens, supports_images, ...}], "count": N}
    let arr = v.get("builtinModels").and_then(|m| m.as_array()).cloned().unwrap_or_default();
    for m in &arr {
        let id = m.get("model_id").and_then(|x| x.as_str()).unwrap_or("").to_string();
        if id.is_empty() { continue; }
        let name = m.get("model_name").and_then(|x| x.as_str()).unwrap_or(&id).to_string();
        out.push(ModelInfo {
            id, name,
            context_window: m.get("context_window").and_then(|x| x.as_u64()),
            max_tokens: m.get("max_tokens").and_then(|x| x.as_u64()),
            credit_multiplier: None,
            supports_images: m.get("supports_images").and_then(|x| x.as_bool()),
        });
    }
    Ok(out)
}
/// Qoder chat：sidecar 签名（常驻进程池）→ POST agent_chat_generation（SSE 双层信封 → OpenAI chunk）
pub async fn qoder_chat_stream(cred: &Cred, body: Value) -> Result<EventStream, String> {
    // 1. 签名（进程池轮转，不再每请求 spawn）
    let signed = qoder_sign(&json!({
        "deviceToken": cred.token,
        "endpoint": "https://gateway.qoder.com.cn",
        "bodyJson": body.to_string(),
        "modelKey": body.get("model").and_then(|m| m.as_str()).unwrap_or("mmodel"),
        "modelSource": "official",
    })).await?;
    let url = signed.get("url").and_then(|u| u.as_str()).ok_or("sidecar: no url")?.to_string();
    let body_str = signed.get("body").and_then(|b| b.as_str()).ok_or("sidecar: no body")?.to_string();
    let headers: Value = signed.get("headers").cloned().unwrap_or(json!({}));
    // 2. 发请求（上游响应是外层信封 SSE：data:{headers,body,statusCode}）
    let mut req = stream_client().post(&url);
    if let Some(hs) = headers.as_object() {
        for (k, v) in hs {
            if let Some(sv) = v.as_str() {
                req = req.header(k, sv);
            }
        }
    }
    let resp = req.body(body_str).send().await.map_err(|e| format!("network: {e}"))?;
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("upstream HTTP {status}: {}", truncate(&text, 400)));
    }
    // 3. 双层信封解析：外层 SSE data:{...body:"<openai chunk json>"...} → 内层 OpenAI delta
    Ok(parse_qoder_envelope_sse(resp.bytes_stream()))
}

/// 仅做 Qoder 签名（诊断/测试用）：返回 sidecar 给出的 url/body/headers 原样。
#[doc(hidden)]
pub async fn qoder_sign_request(cred: &Cred, body: Value) -> Result<Value, String> {
    qoder_sign(&json!({
        "deviceToken": cred.token,
        "endpoint": "https://gateway.qoder.com.cn",
        "bodyJson": body.to_string(),
        "modelKey": body.get("model").and_then(|m| m.as_str()).unwrap_or("mmodel"),
        "modelSource": "official",
    })).await
}

/// Qoder 双层信封 SSE → EventStream
pub fn parse_qoder_envelope_sse(stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static) -> EventStream {
    Box::pin(futures_util::stream::unfold(QoderPump::new(stream), |mut p| async move {
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

/// Qoder 双层信封 SSE 泵（外层 data 帧 → 内层 OpenAI chunk 解码）
struct QoderPump {
    buf: Vec<u8>,
    stream: std::pin::Pin<Box<dyn Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>>,
    done: bool,
    /// 已产出错误事件：后续 event:error 栈帧/兜底错误不再重复上报（只保留根因）
    errored: bool,
}

impl QoderPump {
    fn new(stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static) -> Self {
        Self { buf: Vec::new(), stream: Box::pin(stream), done: false, errored: false }
    }
    fn feed(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }
    fn pop(&mut self) -> Option<StreamEvent> {
        loop {
            let pos = find_block_end(&self.buf)?;
            let block = String::from_utf8_lossy(&self.buf[..pos.0]).to_string();
            self.buf.drain(..pos.1);
            // 帧可能是 "event:error\ndata:{...}" 或纯 "data:{...}"；event 名参与错误判定
            let (event_name, data) = match parse_event_block(&block) {
                Some(x) => x,
                None => continue,
            };
            if event_name == "error" {
                if self.errored {
                    continue;
                }
                self.errored = true;
                let detail: String = data.chars().take(200).collect();
                return Some(StreamEvent::Error(
                    format!("Qoder 上游流错误：{detail}"),
                    "qoder_stream_error".into(),
                ));
            }
            if data.is_empty() { continue; }
            if let Some(ev) = decode_qoder_envelope(&data) {
                if matches!(ev, StreamEvent::Error(..)) {
                    if self.errored {
                        continue;
                    }
                    self.errored = true;
                }
                return Some(ev);
            }
        }
    }
    fn take_tail(&mut self) -> Option<StreamEvent> {
        if self.buf.is_empty() { return None; }
        let block = String::from_utf8_lossy(&self.buf).to_string();
        self.buf.clear();
        let (event_name, data) = parse_event_block(&block)?;
        if event_name == "error" {
            if self.errored {
                return None;
            }
            self.errored = true;
            return Some(StreamEvent::Error(
                format!("Qoder 上游流错误：{}", data.chars().take(200).collect::<String>()),
                "qoder_stream_error".into(),
            ));
        }
        if data.is_empty() { return None; }
        let ev = decode_qoder_envelope(&data);
        if let Some(StreamEvent::Error(..)) = &ev {
            if self.errored {
                return None;
            }
            self.errored = true;
        }
        ev
    }
}

/// Qoder 外层信封状态码 → 数字（statusCodeValue 优先，statusCode 字符串兜底）
fn qoder_http_status(envelope: &Value) -> Option<i64> {
    if let Some(v) = envelope.get("statusCodeValue").and_then(|s| s.as_i64()) {
        return Some(v);
    }
    let s = envelope.get("statusCode").and_then(|s| s.as_str()).unwrap_or("");
    Some(match s {
        "OK" => 200,
        "BAD_REQUEST" => 400,
        "UNAUTHORIZED" => 401,
        "FORBIDDEN" => 403,
        "NOT_FOUND" => 404,
        "TOO_MANY_REQUESTS" => 429,
        "INTERNAL_SERVER_ERROR" => 500,
        "BAD_GATEWAY" => 502,
        "SERVICE_UNAVAILABLE" => 503,
        _ => return None,
    })
}

/// 外层信封 data:{headers, body:"<json>", statusCode} → 内层 OpenAI chunk → 事件。
/// 关键修复：信封内是业务错误（如 invalid_model_error / 额度不足）时，此前会被静默丢弃，
/// 客户端只看到空流/502；现在显式转成 Error 事件，透出上游真实原因。
fn decode_qoder_envelope(data: &str) -> Option<StreamEvent> {
    let envelope: Value = serde_json::from_str(data).ok()?;
    let status = qoder_http_status(&envelope);
    let inner_str = envelope.get("body").and_then(|b| b.as_str()).unwrap_or("");
    if !inner_str.trim().is_empty() {
        match serde_json::from_str::<Value>(inner_str) {
            Ok(inner) => {
                // 业务错误帧：{code:"invalid_model_error", message:"Unsupported model ...", type:...}
                if inner.get("choices").is_none() {
                    if let Some(code) = inner.get("code").and_then(|c| c.as_str()) {
                        let msg = inner
                            .get("message")
                            .or_else(|| inner.get("msg"))
                            .and_then(|m| m.as_str())
                            .unwrap_or("Qoder 上游错误");
                        // 模型不支持时给出可执行提示（客户端可换模型/换平台）
                        let hint = if code == "invalid_model_error" {
                            "（该模型在当前 Qoder 账号不可用，请在 /v1/models 里选其它模型，或在账号路由页切换请求平台）"
                        } else {
                            ""
                        };
                        return Some(StreamEvent::Error(
                            format!("Qoder 上游错误（{code}）：{msg}{hint}"),
                            code.to_string(),
                        ));
                    }
                    // 其它错误结构（success=false 等）
                    if inner.get("success").and_then(|s| s.as_bool()) == Some(false) {
                        let msg = inner
                            .get("message")
                            .or_else(|| inner.get("msgInfo"))
                            .and_then(|m| m.as_str())
                            .unwrap_or("Qoder 上游内部错误");
                        return Some(StreamEvent::Error(format!("Qoder 上游错误：{msg}"), "qoder_error".into()));
                    }
                }
                // 复用 OpenAI chunk 解析（choices/delta 形状一致）
                let events = parse_openai_chunk(&inner);
                if let Some(ev) = events.into_iter().next() {
                    return Some(ev);
                }
                return None;
            }
            Err(_) => {
                // body 是 HTML/纯文本（网关错误页）
                return Some(StreamEvent::Error(
                    format!(
                        "Qoder 上游返回非 JSON（HTTP {}）：{}",
                        status.unwrap_or(200),
                        truncate(inner_str, 200)
                    ),
                    "qoder_bad_body".into(),
                ));
            }
        }
    }
    // body 为空且状态码非 2xx → 状态错误
    if let Some(st) = status {
        if !(200..300).contains(&st) {
            return Some(StreamEvent::Error(format!("Qoder 上游 HTTP {st}（空响应体）"), format!("qoder_http_{st}")));
        }
    }
    None
}

/// 诊断/测试用：解码一帧 Qoder 外层信封
#[doc(hidden)]
pub fn qoder_envelope_event(data: &str) -> Option<StreamEvent> {
    decode_qoder_envelope(data)
}

#[cfg(test)]
mod stream_tests {
    use super::*;

    #[test]
    fn find_block_end_no_panic_on_leading_newline() {
        // 旧实现在 buf 以单个 '\n' 开头时 buf[i-1] 越界 panic
        assert_eq!(find_block_end(b"\ndata: {}"), None);
        assert_eq!(find_block_end(b"\n"), None);
        assert_eq!(find_block_end(b"\n\n"), Some((0, 2)));
        assert_eq!(find_block_end(b"data: {}\n\n"), Some((8, 10)));
        assert_eq!(find_block_end(b"data: {}\r\n\r\ndata: {}"), Some((9, 12)));
        assert_eq!(find_block_end(b"data: {}\n\r\ndata: {}"), Some((8, 11)));
    }

    #[tokio::test]
    async fn pump_finishes_even_if_upstream_never_closes() {
        use futures_util::StreamExt;
        // 先发一个 finish_reason 帧，然后流永远 pending（模拟上游不关闭连接）
        let first = bytes::Bytes::from_static(b"data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n");
        let s = futures_util::stream::once(async move { Ok::<_, reqwest::Error>(first) })
            .chain(futures_util::stream::pending());
        let mut ev = parse_openai_sse(s);
        let t0 = std::time::Instant::now();
        let mut done = false;
        while let Some(item) = ev.next().await {
            if let Ok(StreamEvent::Done { .. }) = item { done = true; }
            if t0.elapsed() > std::time::Duration::from_secs(10) { break; }
        }
        assert!(done, "应收到 Done 事件");
        assert!(t0.elapsed() < std::time::Duration::from_secs(8), "上游不关闭时也应在宽限时间内结束，实际 {:?}", t0.elapsed());
    }

    #[tokio::test]
    async fn pump_emits_all_text_chunks() {
        use futures_util::StreamExt;
        let chunks: Vec<bytes::Bytes> = vec![
            bytes::Bytes::from_static(b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\xe4\xbd\xa0\"},\"finish_reason\":null}]}\n\n"),
            bytes::Bytes::from_static(b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\xe5\xa5\xbd\"},\"finish_reason\":null}]}\n"),
            bytes::Bytes::from_static(b"\ndata: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"),
        ];
        let s = futures_util::stream::iter(chunks.into_iter().map(Ok::<_, reqwest::Error>));
        let mut ev = parse_openai_sse(s);
        let mut text = String::new();
        let mut done = false;
        while let Some(item) = ev.next().await {
            match item.expect("no stream error") {
                StreamEvent::Text(t) => text.push_str(&t),
                StreamEvent::Done { .. } => done = true,
                _ => {}
            }
        }
        assert_eq!(text, "你好");
        assert!(done, "应收到结束事件");
    }
}
