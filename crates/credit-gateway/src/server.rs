//! 网关 HTTP 服务：对外 OpenAI 标准接口 + 管理/账号路由接口。
//!
//! 对外（像本地 Ollama 一样只给接口地址）：
//!   GET  /v1/models                    — 模型清单（合并全部平台，id 形如 平台/模型）
//!   POST /v1/chat/completions          — OpenAI chat completions（stream SSE / 非流式）
//!   GET  /v1/platforms                 — 平台清单 + 当前平台
//!
//! 管理面（UI 账号路由页调用）：
//!   GET/POST /admin/platform           — 查询/切换当前平台
//!   GET/POST /admin/rules              — 路由规则 CRUD
//!   PUT/DELETE /admin/rules/:id
//!   GET  /admin/status                 — 网关状态

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router as AxumRouter};
use futures_util::StreamExt;
use serde_json::{json, Value};

use credit_core::adapters::Cred;
use credit_core::db::Store;

use crate::protocol::{normalize_messages, Platform};
use crate::router::{RouteRule, Router};
use crate::upstream::{self, EventStream, StreamEvent};

pub struct GatewayState {
    pub store: Arc<Store>,
    pub router: Router,
    /// 可选 Bearer 鉴权（空 = 不鉴权，仅本机使用）
    pub api_key: String,
    /// 速率限制：60s 滑动窗口请求时间戳（0 = 不限速）
    pub rate_window: std::sync::Mutex<VecDeque<Instant>>,
    /// 实际监听端口（status 展示用）
    pub port: u16,
    /// WorkBuddy 设备盐（64hex；.workbuddy-device.json 或生成）
    pub device_salt: std::sync::Mutex<String>,
    /// Turing 设备 token 缓存 (token, 拉取时间)
    pub device_token: std::sync::Mutex<Option<(String, std::time::Instant)>>,
}

impl GatewayState {
    /// 每分钟请求数上限（settings gatewayRpm；0 = 不限）
    pub fn rpm_limit(&self) -> u32 {
        self.store.get_setting("gatewayRpm", "0").ok()
            .and_then(|v| v.parse::<u32>().ok()).unwrap_or(0)
    }

    /// 限速检查：true = 放行，false = 已超限
    pub fn rate_allow(&self) -> bool {
        let rpm = self.rpm_limit();
        if rpm == 0 {
            return true;
        }
        let mut w = self.rate_window.lock().unwrap();
        let now = Instant::now();
        while let Some(t) = w.front() {
            if now.duration_since(*t).as_secs() >= 60 {
                w.pop_front();
            } else {
                break;
            }
        }
        if w.len() >= rpm as usize {
            return false;
        }
        w.push_back(now);
        true
    }
}

impl GatewayState {
    fn check_auth(&self, auth_header: Option<&str>) -> bool {
        if self.api_key.is_empty() {
            return true;
        }
        auth_header
            .and_then(|h| h.strip_prefix("Bearer "))
            .map(|k| k == self.api_key)
            .unwrap_or(false)
    }
}

/// Turing 设备 token：跑 dsh-connect-workbuddy 的 worker.cjs（加载本机桌面端 SDK 真机签名）
fn fetch_device_token() -> Option<String> {
    use std::process::Command;
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let sdk_dirs = [
        "D:/ProgramFiles/software/WorkBuddy/resources/app.asar.unpacked/native/turing-sdk".to_string(),
        format!("{local}/Programs/WorkBuddy/resources/app.asar.unpacked/native/turing-sdk"),
        "C:/Program Files/WorkBuddy/resources/app.asar.unpacked/native/turing-sdk".to_string(),
    ];
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    let worker_candidates = [
        format!("{home}/.dsh/profiles/web/node_modules/dsh-connect-workbuddy/native/turing-sdk/worker.cjs"),
        "C:/Users/11833/.dsh/profiles/web/node_modules/dsh-connect-workbuddy/native/turing-sdk/worker.cjs".to_string(),
    ];
    let worker_path = worker_candidates.into_iter().find(|c| std::path::Path::new(c).exists())?;
    for sdk in sdk_dirs {
        if !std::path::Path::new(&sdk).exists() { continue; }
        let out = Command::new("node")
            .arg(&worker_path)
            .arg("token")
            .env("TURING_SDK_DIR", &sdk)
            .env("TURING_CHANNEL_ID", "109144")
            .env("TURING_PRODUCT_NAME", "WorkBuddy")
            .env("TURING_PRODUCT_VERSION", "5.11.3")
            .env("TURING_TIMEOUT_MS", "20000")
            .output()
            .ok()?;
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
        if v.get("ok").and_then(|o| o.as_bool()).unwrap_or(false) {
            return v.get("token").and_then(|t| t.as_str()).map(|s| s.to_string());
        }
    }
    None
}

/// 设备盐初始化：读 DSH 盐文件（与 dsh-connect-workbuddy 同一约定），无则生成并落盘
pub fn device_salt_init() -> String {
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    let path = format!("{home}/.dsh/.workbuddy-device.json");
    if let Ok(v) = std::fs::read_to_string(&path) {
        if let Ok(doc) = serde_json::from_str::<serde_json::Value>(&v) {
            if let Some(sv) = doc.get("salt").and_then(|x| x.as_str()) {
                return sv.to_string();
            }
        }
    }
    // 生成 64hex 盐（randomUUID x2 拼 64 hex）
    let salt = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    let salt = &salt[..64.min(salt.len())];
    let _ = std::fs::create_dir_all(format!("{home}/.dsh"));
    let _ = std::fs::write(&path, serde_json::json!({"version": 1, "salt": salt, "createdAt": credit_core::adapters::now_iso()}).to_string());
    salt.to_string()
}

/// WorkBuddy 设备三元组：(device_token, machine_id, session_id)。盐派生 + token 缓存刷新。
fn wb_device_identity(st: &Arc<GatewayState>, uid: &str) -> (String, String, String) {
    use sha2::{Digest, Sha256};
    let salt = st.device_salt.lock().unwrap().clone();
    let machine = if salt.is_empty() || uid.is_empty() { String::new() } else {
        Sha256::digest(format!("dsh-connect-workbuddy/device/v1\0{salt}\0machine\0{uid}")).iter().map(|b| format!("{b:02x}")).collect::<String>().chars().take(36).collect()
    };
    let session = if salt.is_empty() || uid.is_empty() { String::new() } else {
        Sha256::digest(format!("dsh-connect-workbuddy/device/v1\0{salt}\0session\0{uid}")).iter().map(|b| format!("{b:02x}")).collect::<String>().chars().take(36).collect()
    };
    let mut tok_cache = st.device_token.lock().unwrap();
    let dev_token = match tok_cache.as_ref() {
        Some((t, at)) if at.elapsed() < std::time::Duration::from_secs(300) => t.clone(),
        _ => {
            let fresh = fetch_device_token();
            if let Some(t) = &fresh { *tok_cache = Some((t.clone(), std::time::Instant::now())); }
            fresh.unwrap_or_default()
        }
    };
    drop(tok_cache);
    (dev_token, machine, session)
}

fn err_response(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({ "error": { "message": message, "type": "gateway_error", "code": code } })),
    )
        .into_response()
}

fn openai_error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({ "error": { "message": message, "type": "invalid_request_error", "code": code } })),
    )
        .into_response()
}

pub fn build_router(state: Arc<GatewayState>) -> AxumRouter {
    // 允许任意来源（本地 WebView / 第三方客户端跨端口调用）
    let cors = tower_http::cors::CorsLayer::new()
        .allow_origin(tower_http::cors::Any)
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any);
    AxumRouter::new()
        .route("/v1/models", get(list_models))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/platforms", get(list_platforms))
        .route("/admin/platform", get(get_platform).post(set_platform))
        .route("/admin/rules", get(list_rules).post(add_rule))
        .route("/admin/rules/:id", delete(delete_rule).put(update_rule))
        .route("/admin/status", get(gateway_status))
        .route("/healthz", get(healthz))
        .layer(cors)
        .with_state(state)
}

/// 启动网关监听（Tauri setup / CLI serve 子命令调用）
pub async fn serve(state: Arc<GatewayState>, addr: SocketAddr) -> anyhow::Result<()> {
    let app = build_router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

// ---------- 平台清单 ----------

async fn list_platforms(State(st): State<Arc<GatewayState>>) -> Response {
    let current = st.router.current_platform();
    Json(json!({
        "current": current.as_str(),
        "platforms": Platform::all().iter().map(|p| {
            let accounts = st.router.accounts_for(*p);
            json!({
                "name": p.as_str(),
                "label": p.display(),
                "account_provider": p.account_provider(),
                "accounts": accounts.len(),
            })
        }).collect::<Vec<_>>(),
    }))
    .into_response()
}

// ---------- 模型清单 ----------

async fn list_models(
    State(st): State<Arc<GatewayState>>,
    headers: axum::http::HeaderMap,
) -> Response {
    if !st.check_auth(headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok())) {
        return openai_error_response(StatusCode::UNAUTHORIZED, "unauthorized", "missing or invalid Authorization bearer");
    }
    let mut data = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // 模型目录只返回当前请求平台的模型（跟随「请求平台」切换；模型 id 为裸 id）
    let targets: Vec<Platform> = vec![st.router.current_platform()];
    for platform in targets {
        let Some(cred) = st.router.accounts_for(platform).into_iter().next() else { continue };
        let models = match platform {
            Platform::WorkBuddy | Platform::WorkBuddyIntl => upstream::wb_fetch_models(&cred).await,
            Platform::Trae | Platform::TraeEnterprise => upstream::trae_fetch_models(&cred, false).await,
            Platform::TraeIntl => upstream::trae_fetch_models(&cred, true).await,
            Platform::Zcode => upstream::zcode_fetch_models(&cred).await,
            Platform::Minimax => upstream::minimax_fetch_models(&cred).await,
            Platform::Qoder => upstream::qoder_fetch_models(&cred).await,
            Platform::CodeArts => continue,
        };
        let Ok(models) = models else { continue };
        for m in models {
            let key = m.id.clone();
            if !seen.insert(key.clone()) {
                continue;
            }
            data.push(json!({
                "id": key,
                "object": "model",
                "created": 0,
                "owned_by": platform.as_str(),
                "platform": platform.as_str(),
                "raw_model": m.id,
                "name": m.name,
                "context_window": m.context_window,
                "max_tokens": m.max_tokens,
                "credit_multiplier": m.credit_multiplier,
            }));
        }
    }
    Json(json!({ "object": "list", "data": data })).into_response()
}

// ---------- chat completions ----------

/// 解析 model 字段 → (Platform, 裸模型id)
fn resolve_model(model_field: &str, current: Platform) -> Option<(Platform, String)> {
    let raw = model_field.trim();
    if raw.is_empty() {
        return None;
    }
    if let Some((plat, model)) = raw.split_once('/') {
        if let Some(p) = Platform::parse(plat) {
            if !model.is_empty() {
                return Some((p, model.to_string()));
            }
        }
        return None;
    }
    Some((current, raw.to_string()))
}

async fn chat_completions(
    State(st): State<Arc<GatewayState>>,
    headers: axum::http::HeaderMap,
    body: axum::extract::Json<Value>,
) -> Response {
    if !st.check_auth(headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok())) {
        return openai_error_response(StatusCode::UNAUTHORIZED, "unauthorized", "missing or invalid Authorization bearer");
    }
    // 速率限制（settings gatewayRpm，0 = 不限）
    if !st.rate_allow() {
        let _ = st.store.log_gateway_request(st.router.current_platform().as_str(), "", body.0.get("model").and_then(|m| m.as_str()).unwrap_or(""), false, Some(429), Some("rate limited"), None);
        return openai_error_response(StatusCode::TOO_MANY_REQUESTS, "rate_limited",
            &format!("rate limit exceeded: {} requests/min (settings: gatewayRpm)", st.rpm_limit()));
    }
    let body = body.0;
    let model_field = body.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
    let current = st.router.current_platform();
    let Some((platform, model)) = resolve_model(&model_field, current) else {
        return openai_error_response(StatusCode::BAD_REQUEST, "missing_model",
            "field \"model\" is required: bare id (routed via current platform) or \"platform/model\"");
    };
    // 默认只取当前请求平台：裸 id 固定当前平台；strict 开启时显式 平台/模型 也仅限当前平台
    let strict = st.store.get_setting("gatewayStrict", "1").ok().map(|v| v == "1").unwrap_or(true);
    if strict && platform != current {
        let _ = st.store.log_gateway_request(current.as_str(), "", &model, false, Some(403), Some("platform locked"), None);
        return openai_error_response(StatusCode::FORBIDDEN, "platform_locked",
            &format!("gateway is locked to current platform \"{}\"; requested \"{}\" (在账号路由页关闭\"仅允许当前平台\"可放开)", current.as_str(), platform.as_str()));
    }
    let openai_messages = body.get("messages").and_then(|m| m.as_array()).cloned().unwrap_or_default();
    if openai_messages.is_empty() {
        return openai_error_response(StatusCode::BAD_REQUEST, "missing_messages", "field \"messages\" is required");
    }
    let (messages, system) = normalize_messages(&openai_messages);
    let max_tokens = body.get("max_tokens").and_then(|v| v.as_u64()).map(|v| v as u32)
        .or_else(|| body.get("max_completion_tokens").and_then(|v| v.as_u64()).map(|v| v as u32));
    let reasoning_effort = body.get("reasoning_effort").and_then(|v| v.as_str()).map(|s| s.to_string());
    let stream = body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);
    let _ = body.get("temperature"); // 上游不接受 temperature 的通道直接忽略

    // 路由规则覆盖（模型前缀命中 → 固定平台/账号）；strict 模式下规则仅能固定账号，平台始终 = 当前平台
    let mut platform = platform;
    let mut pinned_account: Option<Cred> = None;
    if let Some(rule) = st.router.match_rule(&model) {
        if !rule.platform.is_empty() && !strict {
            if let Some(p) = Platform::parse(&rule.platform) {
                platform = p;
            }
        }
        if !rule.account.is_empty() {
            pinned_account = st.router.account_by_ref(&rule.account);
        }
    }

    if !platform.chat_capable() {
        return err_response(StatusCode::NOT_IMPLEMENTED, "chat_unavailable",
            &format!("平台 {} 暂无可用的对话通道（该平台仅支持签到与额度管理）", platform.display()));
    }
    let creds: Vec<Cred> = match pinned_account {
        Some(c) => vec![c],
        None => st.router.accounts_for(platform),
    };
    if creds.is_empty() {
        return err_response(StatusCode::SERVICE_UNAVAILABLE, "no_account",
            &format!("平台 {} 没有可用账号，请先在账号管理添加或扫描导入", platform.display()));
    }

    // 逐账号 failover + 11128 渠道风控退避重试（风控抖动时"请重新发送"即可过，自动重试代替手动）
    let mut last_error: Option<(String, String)> = None;
    'outer: for attempt in 0..3u32 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(1500 * attempt as u64)).await;
        }
        for cred in &creds {
            match dispatch_stream(&st, platform, cred, &model, &messages, &system, max_tokens, reasoning_effort.as_deref()).await {
                Ok(events) => {
                    // 请求日志：成功一条（流式在响应完成前先记 ok=1）
                    let _ = st.store.log_gateway_request(
                        platform.as_str(),
                        &format!("{}:{}", cred.provider, cred.uid),
                        &model, true, Some(200), None, None,
                    );
                    return if stream {
                        stream_response(&model, platform.as_str(), events)
                    } else {
                        blocking_response(&model, platform.as_str(), events).await
                    };
                }
                Err((code, msg)) => {
                    let is_channel_block = msg.contains("11128") || msg.contains("Illegal API invocation");
                    let failover_worthy = code == "auth" || code == "rate" || code == "network" || code == "upstream";
                    if failover_worthy && !is_channel_block {
                        // 渠道拦截不算账号故障（账号本身正常，别标 failed）
                        let _ = st.store.update_account_status(&cred.provider, &cred.uid, "failed", Some(msg.as_str()));
                    }
                    last_error = Some((code, msg.clone()));
                    if !failover_worthy {
                        break 'outer;
                    }
                    // 11128：换下一个账号（本轮内 failover），全部账号都拦 → 外层退避重试
                    continue;
                }
            }
        }
    }
    // 请求日志：失败一条
    let (code, msg) = last_error.unwrap_or_else(|| ("upstream".into(), "upstream error".into()));
    let _ = st.store.log_gateway_request(
        platform.as_str(),
        creds.first().map(|c| format!("{}:{}", c.provider, c.uid)).unwrap_or_default().as_str(),
        &model, false, Some(502), Some(&msg), None,
    );
    err_response(StatusCode::BAD_GATEWAY, &code, &msg)
}

/// 单账号派发：返回上游事件流（错误 → (code, msg) 供 failover）
#[allow(clippy::too_many_arguments)]
async fn dispatch_stream(
    st: &Arc<GatewayState>,
    platform: Platform,
    cred: &Cred,
    model: &str,
    messages: &[Value],
    system: &str,
    max_tokens: Option<u32>,
    reasoning_effort: Option<&str>,
) -> Result<EventStream, (String, String)> {
    match platform {
        Platform::WorkBuddy | Platform::WorkBuddyIntl => {
            let body = crate::protocol::wb_chat_body(messages, system, model, max_tokens, reasoning_effort, &cred.uid);
            // 设备身份（同步派生 + token 缓存刷新，避免跨 await 持锁）
            let (dev_token, machine, session) = wb_device_identity(st, &cred.uid);
            let device = if dev_token.is_empty() && machine.is_empty() { None } else { Some((dev_token.as_str(), machine.as_str(), session.as_str())) };
            upstream::wb_chat_stream(cred, body, device).await.map_err(|e| ("upstream".to_string(), e))
        }
        Platform::Trae | Platform::TraeEnterprise => {
            let enterprise = Router::is_enterprise_cred(cred);
            let body = crate::protocol::trae_chat_body(messages, model, max_tokens, reasoning_effort, enterprise);
            upstream::trae_chat_stream(cred, false, enterprise, body).await.map_err(|e| ("upstream".to_string(), e))
        }
        Platform::TraeIntl => {
            let body = crate::protocol::trae_chat_body(messages, model, max_tokens, reasoning_effort, false);
            upstream::trae_chat_stream(cred, true, false, body).await.map_err(|e| ("upstream".to_string(), e))
        }
        Platform::Zcode => {
            let thinking = !matches!(reasoning_effort, Some("off") | Some("none"));
            let body = crate::protocol::zcode_chat_body(messages, system, model, max_tokens, thinking);
            upstream::zcode_chat_stream(cred, body).await.map_err(|e| ("upstream".to_string(), e))
        }
        Platform::Minimax => {
            let body = crate::protocol::minimax_chat_body(messages, system, model, max_tokens);
            upstream::minimax_chat_stream(cred, body).await.map_err(|e| ("upstream".to_string(), e))
        }
        Platform::Qoder => {
            let body = crate::protocol::qoder_chat_body(messages, model);
            upstream::qoder_chat_stream(cred, body).await.map_err(|e| ("upstream".to_string(), e))
        }
        Platform::CodeArts => {
            Err(("chat_unavailable".to_string(), format!("平台 {} 暂无可用的对话通道", platform.display())))
        }
    }
}

/// SSE 流式响应（OpenAI chat.completion.chunk 线格式）
fn stream_response(model: &str, platform: &str, events: EventStream) -> Response {
    let platform = platform.to_string();
    let model = model.to_string();
    let id = format!("chatcmpl-{}", uuid::Uuid::new_v4().simple());
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let prelude = Event::default().data(
        serde_json::to_string(&json!({
            "id": id, "object": "chat.completion.chunk", "created": created,
            "model": model, "system_fingerprint": format!("gateway-{platform}"),
            "choices": [{ "index": 0, "delta": { "role": "assistant", "content": "" }, "finish_reason": null }],
        })).unwrap_or_default(),
    );

    let body_stream = events.map(move |ev| -> Result<Event, std::convert::Infallible> {
        let payload = match ev {
            Ok(StreamEvent::Text(t)) => json!({
                "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
                "choices": [{ "index": 0, "delta": { "content": t }, "finish_reason": null }],
            }),
            Ok(StreamEvent::Reasoning(t)) => json!({
                "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
                "choices": [{ "index": 0, "delta": { "reasoning_content": t }, "finish_reason": null }],
            }),
            Ok(StreamEvent::Usage { input, output, total }) => json!({
                "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
                "choices": [],
                "usage": {
                    "prompt_tokens": input,
                    "completion_tokens": output,
                    "total_tokens": total.unwrap_or(input + output),
                },
            }),
            Ok(StreamEvent::Done { finish }) => json!({
                "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
                "choices": [{ "index": 0, "delta": {}, "finish_reason": normalize_finish(&finish) }],
            }),
            Ok(StreamEvent::Error(msg, code)) => json!({
                "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
                "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
                "error": { "message": msg, "code": code },
            }),
            Err(e) => json!({
                "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
                "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
                "error": { "message": e, "code": "stream_error" },
            }),
        };
        Ok::<_, std::convert::Infallible>(Event::default().data(payload.to_string()))
    });

    let done = futures_util::stream::once(async {
        Ok::<_, std::convert::Infallible>(Event::default().data("[DONE]"))
    });

    Sse::new(futures_util::stream::iter([Ok::<_, std::convert::Infallible>(prelude)])
        .chain(body_stream)
        .chain(done))
        .keep_alive(KeepAlive::default())
        .into_response()
}

fn normalize_finish(finish: &str) -> String {
    match finish {
        "tool-calls" | "tool_calls" => "tool_calls".into(),
        "max-tokens" | "length" => "length".into(),
        "aborted" | "stop" | _ => "stop".into(),
    }
}

/// 非流式：聚合全部事件产出 chat.completion
async fn blocking_response(
    model: &str,
    platform: &str,
    events: EventStream,
) -> Response {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut usage: Option<(u64, u64, Option<u64>)> = None;
    let mut failure: Option<String> = None;

    let mut stream = std::pin::pin!(events);
    while let Some(ev) = stream.next().await {
        match ev {
            Ok(StreamEvent::Text(t)) => text.push_str(&t),
            Ok(StreamEvent::Reasoning(t)) => reasoning.push_str(&t),
            Ok(StreamEvent::Usage { input, output, total }) => usage = Some((input, output, total)),
            Ok(StreamEvent::Done { .. }) => {}
            Ok(StreamEvent::Error(msg, _)) => failure = Some(msg),
            Err(e) => failure = Some(e),
        }
    }
    if let Some(msg) = failure {
        if text.is_empty() {
            return err_response(StatusCode::BAD_GATEWAY, "upstream", &msg);
        }
    }
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (input, output, total) = usage.unwrap_or((0, 0, None));
    let mut message = json!({ "role": "assistant", "content": text });
    if !reasoning.is_empty() {
        message["reasoning_content"] = json!(reasoning);
    }
    Json(json!({
        "id": format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()),
        "object": "chat.completion",
        "created": created,
        "model": model,
        "system_fingerprint": format!("gateway-{platform}"),
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": "stop",
        }],
        "usage": {
            "prompt_tokens": input,
            "completion_tokens": output,
            "total_tokens": total.unwrap_or(input + output),
        },
    }))
    .into_response()
}

// ---------- 管理面 ----------

async fn get_platform(State(st): State<Arc<GatewayState>>) -> Response {
    let current = st.router.current_platform();
    Json(json!({ "current": current.as_str(), "label": current.display() })).into_response()
}

async fn set_platform(State(st): State<Arc<GatewayState>>, body: axum::extract::Json<Value>) -> Response {
    let name = body.0.get("platform").and_then(|p| p.as_str()).unwrap_or("");
    match Platform::parse(name) {
        Some(p) => {
            st.router.set_current_platform(p);
            Json(json!({ "ok": true, "current": p.as_str(), "label": p.display() })).into_response()
        }
        None => openai_error_response(StatusCode::BAD_REQUEST, "unknown_platform",
            &format!("unknown platform; valid: {}", Platform::all().iter().map(|p| p.as_str()).collect::<Vec<_>>().join(", "))),
    }
}

async fn list_rules(State(st): State<Arc<GatewayState>>) -> Response {
    Json(json!({ "rules": st.router.list_rules() })).into_response()
}

async fn add_rule(State(st): State<Arc<GatewayState>>, body: axum::extract::Json<Value>) -> Response {
    let b = body.0;
    let rule = RouteRule {
        id: String::new(),
        model_pattern: b.get("modelPattern").and_then(|v| v.as_str()).unwrap_or("*").to_string(),
        platform: b.get("platform").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        account: b.get("account").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        weight: b.get("weight").and_then(|v| v.as_u64()).unwrap_or(1) as u32,
        enabled: b.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
        created_at: String::new(),
    };
    let saved = st.router.add_rule(rule);
    Json(json!({ "ok": true, "rule": saved })).into_response()
}

async fn update_rule(
    State(st): State<Arc<GatewayState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    body: axum::extract::Json<Value>,
) -> Response {
    let b = body.0;
    let rules = st.router.list_rules();
    let Some(existing) = rules.iter().find(|r| r.id == id) else {
        return err_response(StatusCode::NOT_FOUND, "not_found", "rule not found");
    };
    let rule = RouteRule {
        id: id.clone(),
        model_pattern: b.get("modelPattern").and_then(|v| v.as_str()).unwrap_or(&existing.model_pattern).to_string(),
        platform: b.get("platform").and_then(|v| v.as_str()).unwrap_or(&existing.platform).to_string(),
        account: b.get("account").and_then(|v| v.as_str()).unwrap_or(&existing.account).to_string(),
        weight: b.get("weight").and_then(|v| v.as_u64()).unwrap_or(existing.weight as u64) as u32,
        enabled: b.get("enabled").and_then(|v| v.as_bool()).unwrap_or(existing.enabled),
        created_at: existing.created_at.clone(),
    };
    if st.router.update_rule(rule) {
        Json(json!({ "ok": true })).into_response()
    } else {
        err_response(StatusCode::INTERNAL_SERVER_ERROR, "save_failed", "failed to save rule")
    }
}

async fn delete_rule(State(st): State<Arc<GatewayState>>, axum::extract::Path(id): axum::extract::Path<String>) -> Response {
    if st.router.delete_rule(&id) {
        Json(json!({ "ok": true })).into_response()
    } else {
        err_response(StatusCode::NOT_FOUND, "not_found", "rule not found")
    }
}

async fn gateway_status(State(st): State<Arc<GatewayState>>) -> Response {
    let current = st.router.current_platform();
    let accounts: serde_json::Map<String, Value> = Platform::all().iter()
        .map(|p| (p.as_str().to_string(), json!(st.router.accounts_for(*p).len()))).collect();
    let (total, ok, fail) = st.store.gateway_log_stats().unwrap_or((0, 0, 0));
    Json(json!({
        "platform": current.as_str(),
        "label": current.display(),
        "accounts": accounts,
        "rules": st.router.list_rules().len(),
        "auth": !st.api_key.is_empty(),
        "port": st.port,
        "rpm": st.rpm_limit(),
        "strict": st.store.get_setting("gatewayStrict", "1").ok().map(|v| v == "1").unwrap_or(true),
        "logStats": { "total": total, "ok": ok, "fail": fail },
    }))
    .into_response()
}

async fn healthz() -> Response {
    Json(json!({ "ok": true })).into_response()
}
