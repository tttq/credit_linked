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

use std::net::SocketAddr;
use std::sync::Arc;

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
    AxumRouter::new()
        .route("/v1/models", get(list_models))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/platforms", get(list_platforms))
        .route("/admin/platform", get(get_platform).post(set_platform))
        .route("/admin/rules", get(list_rules).post(add_rule))
        .route("/admin/rules/:id", delete(delete_rule).put(update_rule))
        .route("/admin/status", get(gateway_status))
        .route("/healthz", get(healthz))
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
    for platform in Platform::all() {
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
            let key = format!("{}/{}", platform.as_str(), m.id);
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
    let body = body.0;
    let model_field = body.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
    let Some((platform, model)) = resolve_model(&model_field, st.router.current_platform()) else {
        return openai_error_response(StatusCode::BAD_REQUEST, "missing_model",
            "field \"model\" is required: bare id (routed via current platform) or \"platform/model\"");
    };
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

    // 路由规则覆盖（模型前缀命中 → 固定平台/账号）
    let mut platform = platform;
    let mut pinned_account: Option<Cred> = None;
    if let Some(rule) = st.router.match_rule(&model) {
        if !rule.platform.is_empty() {
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

    // 逐账号 failover
    let mut last_error: Option<(String, String)> = None;
    for cred in &creds {
        match dispatch_stream(&st, platform, cred, &model, &messages, &system, max_tokens, reasoning_effort.as_deref()).await {
            Ok(events) => {
                return if stream {
                    stream_response(&model, platform.as_str(), events)
                } else {
                    blocking_response(&model, platform.as_str(), events).await
                };
            }
            Err((code, msg)) => {
                let failover_worthy = code == "auth" || code == "rate" || code == "network" || code == "upstream";
                if failover_worthy {
                    let _ = st.store.update_account_status(&cred.provider, &cred.uid, "failed", Some(msg.as_str()));
                }
                last_error = Some((code, msg));
                if !failover_worthy {
                    break;
                }
            }
        }
    }
    let (code, msg) = last_error.unwrap_or_else(|| ("upstream".into(), "upstream error".into()));
    err_response(StatusCode::BAD_GATEWAY, &code, &msg)
}

/// 单账号派发：返回上游事件流（错误 → (code, msg) 供 failover）
#[allow(clippy::too_many_arguments)]
async fn dispatch_stream(
    _st: &Arc<GatewayState>,
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
            let body = crate::protocol::wb_chat_body(messages, system, model, max_tokens, reasoning_effort);
            upstream::wb_chat_stream(cred, body).await.map_err(|e| ("upstream".to_string(), e))
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
    Json(json!({
        "platform": current.as_str(),
        "label": current.display(),
        "accounts": accounts,
        "rules": st.router.list_rules().len(),
        "auth": !st.api_key.is_empty(),
    }))
    .into_response()
}

async fn healthz() -> Response {
    Json(json!({ "ok": true })).into_response()
}
