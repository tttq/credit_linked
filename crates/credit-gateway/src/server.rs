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
    /// 模型目录缓存（全平台聚合；外部 Agent 频繁拉 /v1/models，避免每次打上游）
    pub models_cache: std::sync::Mutex<Option<(Instant, serde_json::Value)>>,
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

/// 从本机重新同步凭据（各平台 token 由本地客户端持续轮换）：
/// ① trae/workbuddy/qoder：先重扫本机登录态，失败再走 refresh 链（refresh_token）；
/// ② minimax/zcode/codearts：读客户端存储；zcode/码道 token 未变时自动拉起客户端等待刷新。
/// 返回 true = 已拿到新凭据。
async fn refresh_creds_from_local(st: &Arc<GatewayState>, platform: Platform, stale: &Cred) -> bool {
    use credit_core::service::Service;
    let svc = Service { store: st.store.clone() };
    match platform {
        Platform::Minimax => svc.import_account("minimax", false).await.is_ok(),
        // Trae（国内/企业/国际）、WorkBuddy、Qoder：本地登录态扫描 + refresh_token 刷新链
        Platform::Trae | Platform::TraeEnterprise | Platform::TraeIntl
        | Platform::WorkBuddy | Platform::WorkBuddyIntl | Platform::Qoder => {
            let (provider, intl) = match platform {
                Platform::Trae | Platform::TraeEnterprise => ("trae", false),
                Platform::TraeIntl => ("trae", false), // 本地扫描只识别国内版；国际版走 refresh
                Platform::WorkBuddyIntl => ("workbuddy", true),
                Platform::WorkBuddy => ("workbuddy", false),
                _ => ("qoder", false),
            };
            if svc.import_account(provider, intl).await.is_ok() {
                return true;
            }
            // 扫描失败（如国际版）→ 走 refresh_token 链
            svc.refreshed_cred(&stale.provider, &stale.uid, stale).await.is_ok()
        }
        Platform::Zcode => {
            let stale_token = stale.token.clone();
            // 1) 本地磁盘 token 已变化（客户端在线轮换过）→ 直接导入
            if let Some(l) = credit_core::adapters::zcode_local::read_live_auth() {
                if l.access_token != stale_token {
                    return svc.import_account("zcode", false).await.is_ok();
                }
            }
            // 2) token 仍是旧的 → 拉起本机客户端，等它自动刷新再导入
            let old_fp = credit_core::adapters::zcode_local::token_fingerprint();
            match credit_core::adapters::zcode_local::launch_client() {
                Some(exe) => eprintln!("[gateway] zcode token 过期，已拉起客户端 {}，等待其刷新…", exe.display()),
                None => {
                    eprintln!("[gateway] zcode token 过期且本机未安装 ZCode 客户端");
                    return false;
                }
            }
            let wait = tokio::task::spawn_blocking(move || {
                credit_core::adapters::zcode_local::wait_for_token_change(old_fp.as_deref(), std::time::Duration::from_secs(15))
            }).await.ok().flatten();
            match wait {
                Some(_) => svc.import_account("zcode", false).await.is_ok(),
                None => {
                    eprintln!("[gateway] zcode 客户端 15s 内未刷新 token（可能需要在客户端里重新登录）");
                    false
                }
            }
        }
        Platform::CodeArts => {
            let desktop = stale.provider == "codearts-desktop";
            let read = || credit_core::adapters::codearts_local::read_live_session(desktop)
                .or_else(|| credit_core::adapters::codearts_local::read_live_session(!desktop));
            let stale_st = stale.meta.get("security_token").and_then(|v| v.as_str()).unwrap_or("").to_string();
            // 1) 本地会话已更新 → 直接导入
            if let Some(l) = read() {
                let cur = l.session.pointer("/session/securityToken").and_then(|v| v.as_str()).unwrap_or("");
                if !cur.is_empty() && cur != stale_st {
                    return svc.import_account(&stale.provider, false).await.is_ok();
                }
            }
            // 2) 拉起本机客户端等刷新
            let old_fp = credit_core::adapters::codearts_local::session_fingerprint(desktop);
            match credit_core::adapters::codearts_local::launch_client() {
                Some(exe) => eprintln!("[gateway] 码道 STS 过期，已拉起客户端 {}，等待其刷新…", exe.display()),
                None => {
                    eprintln!("[gateway] 码道 STS 过期且本机未安装 CodeArts 客户端");
                    return false;
                }
            }
            let wait = tokio::task::spawn_blocking(move || {
                credit_core::adapters::codearts_local::wait_for_session_change(desktop, old_fp.as_deref(), std::time::Duration::from_secs(15))
            }).await.ok().flatten();
            match wait {
                Some(_) => svc.import_account(&stale.provider, false).await.is_ok(),
                None => {
                    eprintln!("[gateway] 码道客户端 15s 内未刷新 STS（可能需要在客户端里重新登录）");
                    false
                }
            }
        }
    }
}

/// 拉取一个平台的模型列表（含码道双通道兜底）
async fn fetch_models_for(st: &Arc<GatewayState>, platform: Platform, raw: &str) -> Option<Vec<upstream::ModelInfo>> {
    let all = st.router.accounts_for(platform);
    let prefer = |desktop: bool| -> Vec<Cred> {
        if raw == "codearts-desktop" {
            if desktop {
                all.iter().filter(|c| c.provider == "codearts-desktop").cloned().collect()
            } else {
                all.iter().filter(|c| c.provider != "codearts-desktop").cloned().collect()
            }
        } else {
            all.iter().filter(|c| (c.provider == "codearts-desktop") == desktop).cloned().collect()
        }
    };
    let pool = prefer(true);
    let fallback = prefer(false);
    let mut cred = pool.first().cloned();
    if cred.is_none() {
        cred = fallback.first().cloned();
    }
    let cred = cred?;
    let desktop_channel = cred.provider == "codearts-desktop";
    // 模型目录拉取失败（多为 token 过期）时，先走凭据恢复链（本地重扫 / refresh_token）再试一次
    let mut cred = cred;
    let mut retried = false;
    loop {
        let out = match platform {
            Platform::WorkBuddy | Platform::WorkBuddyIntl => upstream::wb_fetch_models(&cred).await,
            Platform::Trae | Platform::TraeEnterprise => upstream::trae_fetch_models(&cred, false).await,
            Platform::TraeIntl => upstream::trae_fetch_models(&cred, true).await,
            Platform::Zcode => upstream::zcode_fetch_models(&cred).await,
            Platform::Minimax => upstream::minimax_fetch_models(&cred).await,
            Platform::Qoder => upstream::qoder_fetch_models(&cred).await,
            Platform::CodeArts => upstream::codearts_fetch_models(&cred, desktop_channel).await,
        };
        match out {
            Ok(ms) => return Some(ms),
            Err(e) => {
                let auth_like = matches!(&e, credit_core::error::PlatformError::Auth)
                    || e.to_string().contains("401") || e.to_string().contains("403");
                if auth_like && !retried {
                    retried = true;
                    if let Ok(nc) = refresh_creds_for(&st, platform, &cred).await {
                        cred = nc;
                        continue;
                    }
                }
                // 码道：当前通道失败用另一通道凭据兜底（同账号共用 STS 会话）
                if platform == Platform::CodeArts {
                    if let Some(alt) = fallback.first() {
                        return upstream::codearts_fetch_models(alt, alt.provider == "codearts-desktop").await.ok();
                    }
                }
                return None;
            }
        }
    }
}

/// 静默刷新指定平台凭据（不弹窗、不阻塞太久），返回新凭据
async fn refresh_creds_for(st: &Arc<GatewayState>, platform: Platform, stale: &Cred) -> Result<Cred, ()> {
    use credit_core::service::Service;
    let svc = Service { store: st.store.clone() };
    match platform {
        Platform::Trae | Platform::TraeEnterprise | Platform::TraeIntl => {
            let intl = platform == Platform::TraeIntl;
            if svc.import_account("trae", intl).await.is_ok() {
                if let Some(c) = st.router.accounts_for(platform).into_iter().find(|c| c.token != stale.token) {
                    return Ok(c);
                }
            }
            svc.refreshed_cred(&stale.provider, &stale.uid, stale).await.map_err(|_| ())
        }
        Platform::WorkBuddy | Platform::WorkBuddyIntl => {
            let intl = platform == Platform::WorkBuddyIntl;
            if svc.import_account("workbuddy", intl).await.is_ok() {
                if let Some(c) = st.router.accounts_for(platform).into_iter().find(|c| c.token != stale.token) {
                    return Ok(c);
                }
            }
            svc.refreshed_cred(&stale.provider, &stale.uid, stale).await.map_err(|_| ())
        }
        Platform::Qoder => {
            if svc.import_account("qoder", false).await.is_ok() {
                if let Some(c) = st.router.accounts_for(platform).into_iter().find(|c| c.token != stale.token) {
                    return Ok(c);
                }
            }
            svc.refreshed_cred(&stale.provider, &stale.uid, stale).await.map_err(|_| ())
        }
        _ => {
            if refresh_creds_from_local(st, platform, stale).await {
                st.router.accounts_for(platform).into_iter().next().ok_or(())
            } else {
                Err(())
            }
        }
    }
}

/// 构建全平台模型目录（当前平台裸 id；其它平台 "平台/模型"，外部 Agent 可直接选）
/// strict（仅允许当前平台）开启时只列当前平台，避免选到会被 403 的模型。
async fn build_models_catalog(st: &Arc<GatewayState>) -> serde_json::Value {
    let current_raw = st.router.current_platform_raw();
    let strict = st.store.get_setting("gatewayStrict", "1").ok().map(|v| v == "1").unwrap_or(true);
    // 聚合所有「有账号」的平台（含码道双通道）
    let mut targets: Vec<(Platform, String)> = Vec::new();
    for p in Platform::all() {
        let n = st.router.accounts_for(p).len();
        if n == 0 { continue; }
        if p == Platform::CodeArts {
            let all = st.router.accounts_for(p);
            let has_agent = all.iter().any(|c| c.provider == "codearts");
            let has_desktop = all.iter().any(|c| c.provider == "codearts-desktop");
            if has_agent { targets.push((p, "codearts".to_string())); }
            if has_desktop { targets.push((p, "codearts-desktop".to_string())); }
        } else {
            targets.push((p, p.as_str().to_string()));
        }
    }
    // strict：只保留当前通道；否则当前平台排首位
    if strict {
        targets.retain(|(_, raw)| raw == &current_raw);
    } else {
        targets.sort_by_key(|(_, raw)| if raw == &current_raw { 0 } else { 1 });
    }

    let mut data = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (platform, raw) in targets {
        let Some(models) = fetch_models_for(st, platform, &raw).await else { continue };
        let is_current = raw == current_raw;
        for m in models {
            // 当前平台用裸 id（兼容 UI/旧客户端）；其它平台加前缀避免跨平台重名
            let id = if is_current { m.id.clone() } else { format!("{raw}/{}", m.id) };
            if !seen.insert(id.clone()) { continue; }
            data.push(json!({
                "id": id,
                "object": "model",
                "created": 0,
                "owned_by": raw,
                "platform": raw,
                "raw_model": m.id,
                "name": m.name,
                "context_window": m.context_window,
                "max_tokens": m.max_tokens,
                "credit_multiplier": m.credit_multiplier,
            }));
        }
    }
    json!({ "object": "list", "data": data })
}

async fn list_models(
    State(st): State<Arc<GatewayState>>,
    headers: axum::http::HeaderMap,
) -> Response {
    if !st.check_auth(headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok())) {
        return openai_error_response(StatusCode::UNAUTHORIZED, "unauthorized", "missing or invalid Authorization bearer");
    }
    // 60s 缓存（外部 Agent 会频繁拉模型列表；上游拉取含网络请求与 sidecar spawn）
    if let Ok(g) = st.models_cache.lock() {
        if let Some((at, cached)) = &*g {
            if at.elapsed() < std::time::Duration::from_secs(60) {
                return Json(cached.clone()).into_response();
            }
        }
    }
    let catalog = build_models_catalog(&st).await;
    if let Ok(mut g) = st.models_cache.lock() {
        *g = Some((Instant::now(), catalog.clone()));
    }
    Json(catalog).into_response()
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
    // 请求开始时间：用于日志回填「耗时」
    let t0 = std::time::Instant::now();
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
    // Agent 客户端（Cline/Roo/DSH/Cherry 等）依赖的 function calling 字段
    let tools: Vec<Value> = body.get("tools").and_then(|t| t.as_array()).cloned().unwrap_or_default();
    let tool_choice = body.get("tool_choice").cloned();
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
    // 凭据自愈每个请求最多尝试一次（避免拉起客户端 + 等待刷新重复阻塞）
    let mut healed = false;
    'outer: for attempt in 0..3u32 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(1500 * attempt as u64)).await;
        }
        for cred in &creds {
            let mut attempt_cred = cred.clone();
            let mut outcome = dispatch_stream(&st, platform, &attempt_cred, &model, &messages, &system, max_tokens, reasoning_effort.as_deref(), &tools, tool_choice.as_ref()).await;
            // 凭据自愈：minimax/zcode/码道的 token 由本机客户端持续轮换，401 时重读客户端存储
            // （token 未变则自动拉起客户端并等其刷新）再试一次
            if let Err((code, msg)) = &outcome {
                let is_auth = code == "auth" || msg.contains("401") || msg.contains("Unauthorized")
                    || msg.contains("令牌已过期") || msg.contains("AUTH:") || msg.contains("1806")
                    || msg.contains("APIG.0602");
                if is_auth && !healed && matches!(platform,
                    Platform::Minimax | Platform::Zcode | Platform::CodeArts
                    | Platform::Trae | Platform::TraeEnterprise | Platform::TraeIntl
                    | Platform::WorkBuddy | Platform::WorkBuddyIntl | Platform::Qoder) {
                    healed = true;
                    if refresh_creds_from_local(&st, platform, &attempt_cred).await {
                        let rows = st.store.list_accounts().unwrap_or_default();
                        // 自愈后凭据可能换了 uid（如 ZCode 重登）——优先取 token 与旧凭据不同的那行，
                        // 其次回退到同 uid 重新加载
                        let pick = rows.iter()
                            .filter(|(p, _, _, _, _)| p == &attempt_cred.provider)
                            .find_map(|(_, _, _, _, auth)| serde_json::from_str::<Cred>(auth).ok())
                            .filter(|c| c.token != attempt_cred.token)
                            .or_else(|| rows.iter()
                                .find(|(p, u, _, _, _)| p == &attempt_cred.provider && u == &attempt_cred.uid)
                                .and_then(|(_, _, _, _, auth)| serde_json::from_str::<Cred>(auth).ok()));
                        if let Some(nc) = pick {
                            attempt_cred = nc;
                            outcome = dispatch_stream(&st, platform, &attempt_cred, &model, &messages, &system, max_tokens, reasoning_effort.as_deref(), &tools, tool_choice.as_ref()).await;
                        }
                    }
                }
            }
            match outcome {
                Ok(events) => {
                    return if stream {
                        // 流式：先记一条开始行（请求时间），耗时/结果由 StreamGuard 在流结束时回填
                        let account_ref = format!("{}:{}", attempt_cred.provider, attempt_cred.uid);
                        let log_id = st.store.log_gateway_request(
                            platform.as_str(), &account_ref, &model, true, Some(200), None, None,
                        ).unwrap_or(0);
                        stream_response(&st, &model, platform.as_str(), events, log_id, t0)
                    } else {
                        // 非流式：聚合完成后再按实际内容记录（空流记失败，避免"假成功"）
                        let (resp, has_content) = blocking_response(&model, platform.as_str(), events).await;
                        let log_id = st.store.log_gateway_request(
                            platform.as_str(),
                            &format!("{}:{}", attempt_cred.provider, attempt_cred.uid),
                            &model, has_content, Some(if has_content { 200 } else { 502 }),
                            if has_content { None } else { Some("empty stream (no finish_reason/content)") },
                            None,
                        ).unwrap_or(0);
                        let _ = st.store.finish_gateway_log(log_id, t0.elapsed().as_millis() as i64, None, None, None);
                        resp
                    };
                }
                Err((code, msg)) => {
                    let is_channel_block = msg.contains("11128") || msg.contains("Illegal API invocation");
                    // 额度耗尽也视作可切换（各平台错误文案不一）
                    let quota_exhausted = msg.contains("额度") || msg.contains("不足") || msg.contains("exhaust")
                        || msg.contains("quota") || msg.contains("credit") || msg.contains("insufficient")
                        || msg.contains("429") || msg.contains("rate_limit");
                    let failover_worthy = code == "auth" || code == "rate" || code == "network" || code == "upstream" || quota_exhausted;
                    if failover_worthy && !is_channel_block {
                        // 渠道拦截不算账号故障（账号本身正常，别标 failed）
                        let _ = st.store.update_account_status(&attempt_cred.provider, &attempt_cred.uid, "failed", Some(msg.as_str()));
                        // 自动切换：当前账号不可用时，把选中账号切到同平台下一个可用账号
                        let cur_ref = format!("{}:{}", attempt_cred.provider, attempt_cred.uid);
                        let selected = st.store.get_setting("gw_account", "").unwrap_or_default();
                        if selected == cur_ref || selected.is_empty() {
                            if let Some(next) = creds.iter().find(|c| {
                                let r = format!("{}:{}", c.provider, c.uid);
                                r != cur_ref && !c.token.is_empty()
                            }) {
                                let next_ref = format!("{}:{}", next.provider, next.uid);
                                let _ = st.store.set_setting("gw_account", &next_ref);
                                eprintln!("[gateway] 账号 {} 不可用，自动切换到 {}", cur_ref, next_ref);
                            }
                        }
                    }
                    let mut final_msg = msg.clone();
                    // 凭据类平台 + 认证类错误：补一句可执行指引（自愈已尝试拉起客户端）
                    let auth_like = code == "auth" || msg.contains("401") || msg.contains("Unauthorized")
                        || msg.contains("令牌已过期") || msg.contains("AUTH:") || msg.contains("1806")
                        || msg.contains("APIG.0602");
                    if auth_like {
                        final_msg = match platform {
                            Platform::Zcode => format!("{msg} | 已自动同步并尝试拉起本机 ZCode 客户端刷新令牌；若仍失败，请在 ZCode 客户端重新登录，再点账号管理的「扫描本机」"),
                            Platform::CodeArts => format!("{msg} | 已自动同步并尝试拉起本机码道客户端刷新 STS；若仍失败，请在码道客户端重新登录，再点账号管理的「扫描本机」"),
                            _ => final_msg,
                        };
                    }
                    last_error = Some((code, final_msg));
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
    tools: &[Value],
    tool_choice: Option<&Value>,
) -> Result<EventStream, (String, String)> {
    match platform {
        Platform::WorkBuddy | Platform::WorkBuddyIntl => {
            let mut body = crate::protocol::wb_chat_body(messages, system, model, max_tokens, reasoning_effort, &cred.uid);
            crate::protocol::attach_openai_tools(&mut body, tools, tool_choice);
            // 设备身份（同步派生 + token 缓存刷新，避免跨 await 持锁）
            let (dev_token, machine, session) = wb_device_identity(st, &cred.uid);
            let device = if dev_token.is_empty() && machine.is_empty() { None } else { Some((dev_token.as_str(), machine.as_str(), session.as_str())) };
            upstream::wb_chat_stream(cred, body, device).await.map_err(|e| ("upstream".to_string(), e))
        }
        Platform::Trae | Platform::TraeEnterprise | Platform::TraeIntl => {
            let intl = platform == Platform::TraeIntl;
            let enterprise = platform == Platform::TraeEnterprise || (platform == Platform::Trae && Router::is_enterprise_cred(cred));
            // 模型只在列出它的 function 下可调用：先用模型目录映射的 function，再按优先级回退
            let mut funcs: Vec<String> = Vec::new();
            if let Some(f) = upstream::trae_model_function(model) { funcs.push(f); }
            for f in ["solo_work_lite", "solo_work_remote", "chat_v3", "solo_coder", "solo_agent_remote"] {
                if !funcs.iter().any(|x| x == f) { funcs.push(f.to_string()); }
            }
            let mut last: Result<EventStream, (String, String)> =
                Err(("upstream".to_string(), "trae: no attempt".to_string()));
            for f in funcs {
                let mut body = crate::protocol::trae_chat_body(messages, model, &f, max_tokens, reasoning_effort, enterprise);
                crate::protocol::attach_trae_tools(&mut body, tools);
                match upstream::trae_chat_stream(cred, intl, enterprise, body).await {
                    Ok(s) => return Ok(s),
                    Err(e) => {
                        // 4001 = function/config_name 不匹配，换下一个 function；其它错误立即返回
                        if e.contains("4001") || e.contains("config_name") {
                            last = Err(("upstream".to_string(), e));
                            continue;
                        }
                        return Err(("upstream".to_string(), e));
                    }
                }
            }
            last
        }
        Platform::Zcode => {
            let thinking = !matches!(reasoning_effort, Some("off") | Some("none"));
            let mut body = crate::protocol::zcode_chat_body(messages, system, model, max_tokens, thinking);
            crate::protocol::attach_anthropic_tools(&mut body, tools, tool_choice);
            upstream::zcode_chat_stream(cred, body).await.map_err(|e| ("upstream".to_string(), e))
        }
        Platform::Minimax => {
            let mut body = crate::protocol::minimax_chat_body(messages, system, model, max_tokens);
            crate::protocol::attach_anthropic_tools(&mut body, tools, tool_choice);
            upstream::minimax_chat_stream(cred, body).await.map_err(|e| ("upstream".to_string(), e))
        }
        Platform::Qoder => {
            let mut body = crate::protocol::qoder_chat_body(messages, model);
            crate::protocol::attach_openai_tools(&mut body, tools, tool_choice);
            upstream::qoder_chat_stream(cred, body).await.map_err(|e| ("upstream".to_string(), e))
        }
        Platform::CodeArts => {
            // 码道对话：desktop/IDE 通道头区分，凭据共用同一 STS 会话
            let desktop = cred.provider == "codearts-desktop";
            let mut body = crate::protocol::codearts_chat_body(messages, system, model, max_tokens, reasoning_effort.as_deref());
            crate::protocol::attach_openai_tools(&mut body, tools, tool_choice);
            // 码道上游限制「并发会话数 ≤ 3」：按账号做并发闸门（最多 2 个在途），其余排队，
            // 避免 Agent 并发调用时被打满返回 TM.00001041（表现为请求失败/断掉）
            let sem = account_semaphore(&format!("codearts:{}", cred.uid), 2);
            let permit = sem.acquire_owned().await.map_err(|_| ("upstream".to_string(), "codearts 并发闸门已关闭".to_string()))?;
            let events = upstream::codearts_chat_stream(cred, body, desktop).await.map_err(|e| ("upstream".to_string(), e))?;
            // permit 随流存活：流读完/断开前不释放槽位
            Ok(Box::pin(futures_util::StreamExt::map(events, move |ev| {
                let _ = &permit;
                ev
            })))
        }
    }
}

/// 按账号的在途请求信号量（key = provider:uid）。用于上游有并发上限的平台（如码道 3 并发）。
fn account_semaphore(key: &str, permits: usize) -> Arc<tokio::sync::Semaphore> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static MAP: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Semaphore>>>> = OnceLock::new();
    let map = MAP.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = map.lock().unwrap();
    g.entry(key.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(permits)))
        .clone()
}

/// 流式请求的收尾守卫：无论流正常结束还是被客户端中断，都回填耗时/结果到请求日志。
struct StreamGuard {
    store: Arc<credit_core::db::Store>,
    log_id: i64,
    t0: std::time::Instant,
    finished: Arc<std::sync::atomic::AtomicBool>,
    error: Arc<std::sync::Mutex<Option<String>>>,
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        let elapsed = self.t0.elapsed().as_millis() as i64;
        let finished = self.finished.load(std::sync::atomic::Ordering::Relaxed);
        if finished {
            let _ = self.store.finish_gateway_log(self.log_id, elapsed, None, None, None);
        } else {
            let msg = self.error.lock().ok().and_then(|g| g.clone())
                .unwrap_or_else(|| "客户端中断或流未正常结束".to_string());
            let _ = self.store.finish_gateway_log(self.log_id, elapsed, Some(false), Some(502), Some(&msg));
        }
    }
}

/// 修复上游工具参数伪影：部分上游（实测 Qoder gfmodel）流式分词时会在**每个值后面**多吐一个
/// `""`，例如 `{"limit":75"","offset":50""}`、`{"limit":100""}`。
/// 客户端会因非法 JSON 报 "Expected ',' or '}' after property value"（工具调用直接失败）。
/// 处理原则：`""` 前一个有效字符是 `: , [ {` 时是合法的空字符串，保留；
/// 若前一个字符是数字/true/false/null/`}`/`]`（完整值后紧跟引号）说明两个引号都是伪影，删掉；
/// 其它情况（字符串结尾多一个引号）删掉一个。校验通过即返回，修不好则原样返回。
fn sanitize_tool_arguments(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() || serde_json::from_str::<serde_json::Value>(raw).is_ok() {
        return raw.to_string();
    }
    let mut cur = raw.to_string();
    for _ in 0..16 {
        if serde_json::from_str::<serde_json::Value>(&cur).is_ok() {
            return cur;
        }
        let chars: Vec<char> = cur.chars().collect();
        let mut out = String::with_capacity(cur.len());
        let mut changed = false;
        let mut i = 0;
        while i < chars.len() {
            if i + 1 < chars.len() && chars[i] == '"' && chars[i + 1] == '"' {
                let prev = out.chars().rev().find(|c| !c.is_whitespace());
                match prev {
                    // 合法空字符串（"key":"" / [""] / ,""）
                    Some(':') | Some(',') | Some('[') | Some('{') | None => {
                        out.push('"');
                        out.push('"');
                    }
                    // 完整值后紧跟引号（如 75"" / true"" / }"" ）：两个都丢掉
                    Some(c) if c.is_ascii_digit() || c == '}' || c == ']'
                        || c == 'e' || c == 'l' => {}
                    // 字符串结尾多一个引号：只丢一个
                    _ => {
                        out.push('"');
                    }
                }
                changed = true;
                i += 2;
                continue;
            }
            out.push(chars[i]);
            i += 1;
        }
        if !changed {
            break;
        }
        cur = out;
    }
    if serde_json::from_str::<serde_json::Value>(&cur).is_ok() {
        cur
    } else {
        raw.to_string()
    }
}

/// SSE 流式响应（OpenAI chat.completion.chunk 线格式）
fn stream_response(
    st: &Arc<GatewayState>,
    model: &str,
    platform: &str,
    events: EventStream,
    log_id: i64,
    t0: std::time::Instant,
) -> Response {
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

    // 工具调用计数：上游常以 finish=stop 结束但携带 tool_calls，Agent 客户端要求 finish=tool_calls
    let tool_seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let tool_seen2 = tool_seen.clone();
    // 流结束状态（守卫在流被 drop 时回填日志）
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let finished2 = finished.clone();
    let error = Arc::new(std::sync::Mutex::new(None::<String>));
    let error2 = error.clone();
    let guard = StreamGuard {
        store: st.store.clone(),
        log_id,
        t0,
        finished,
        error,
    };

    // 工具参数按 index 缓冲：上游分片可能含伪影（如 Qoder 多发 `""`），
    // 直接透传会让客户端拼出非法 JSON；这里缓存分片，在结束事件时校验/修复后一次性下发。
    let mut tool_buf: std::collections::BTreeMap<u32, (Option<String>, String, String)> = Default::default();
    let mut tool_flushed = false;

    let body_stream = events.flat_map(move |ev| {
        // guard 随闭包存活 → 流结束/中断时 Drop 回填耗时
        let _keep_alive = &guard;
        let mut out: Vec<Result<Event, std::convert::Infallible>> = Vec::new();
        match &ev {
            Ok(StreamEvent::Done { .. }) => finished2.store(true, std::sync::atomic::Ordering::Relaxed),
            Ok(StreamEvent::ToolCall { .. }) => tool_seen2.store(true, std::sync::atomic::Ordering::Relaxed),
            Ok(StreamEvent::Error(m, _)) | Err(m) => {
                if let Ok(mut g) = error2.lock() { *g = Some(m.clone()); }
            }
            _ => {}
        }
        match ev {
            Ok(StreamEvent::Text(t)) => out.push(Ok(Event::default().data(json!({
                "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
                "choices": [{ "index": 0, "delta": { "content": t }, "finish_reason": null }],
            }).to_string()))),
            Ok(StreamEvent::Reasoning(t)) => out.push(Ok(Event::default().data(json!({
                "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
                "choices": [{ "index": 0, "delta": { "reasoning_content": t }, "finish_reason": null }],
            }).to_string()))),
            Ok(StreamEvent::ToolCall { index, id: tool_id, name, arguments }) => {
                let e = tool_buf.entry(index).or_default();
                // id 只在下发一次
                if let Some(tid) = &tool_id {
                    if e.0.is_none() {
                        e.0 = Some(tid.clone());
                        let tc = json!({ "index": index, "id": tid, "type": "function" });
                        out.push(Ok(Event::default().data(json!({
                            "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
                            "choices": [{ "index": 0, "delta": { "tool_calls": [tc] }, "finish_reason": null }],
                        }).to_string())));
                    }
                }
                // name 按上游分片原样透传（客户端自身会拼接；重复整名会得到 readread）
                if let Some(n) = &name {
                    if !n.is_empty() {
                        e.1.push_str(n);
                        let tc = json!({ "index": index, "function": { "name": n } });
                        out.push(Ok(Event::default().data(json!({
                            "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
                            "choices": [{ "index": 0, "delta": { "tool_calls": [tc] }, "finish_reason": null }],
                        }).to_string())));
                    }
                }
                // 参数缓存到结束帧统一校验/修复后下发
                if let Some(a) = arguments { e.2.push_str(&a); }
            }
            Ok(StreamEvent::Usage { input, output, total }) => out.push(Ok(Event::default().data(json!({
                "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
                "choices": [],
                "usage": {
                    "prompt_tokens": input,
                    "completion_tokens": output,
                    "total_tokens": total.unwrap_or(input + output),
                },
            }).to_string()))),
            Ok(StreamEvent::Done { finish }) => {
                if !tool_flushed {
                    tool_flushed = true;
                    for (index, (_tid, _name, args)) in &tool_buf {
                        let args_str = if args.trim().is_empty() {
                            "{}".to_string()
                        } else {
                            sanitize_tool_arguments(args)
                        };
                        // 只补参数（id/name 已在前面的分片下发，重复会拼成 readread）
                        let tc = json!({ "index": index, "function": { "arguments": args_str } });
                        out.push(Ok(Event::default().data(json!({
                            "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
                            "choices": [{ "index": 0, "delta": { "tool_calls": [tc] }, "finish_reason": null }],
                        }).to_string())));
                    }
                }
                let mut fr = normalize_finish(&finish);
                if fr == "stop" && tool_seen2.load(std::sync::atomic::Ordering::Relaxed) { fr = "tool_calls".to_string(); }
                out.push(Ok(Event::default().data(json!({
                    "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
                    "choices": [{ "index": 0, "delta": {}, "finish_reason": fr }],
                }).to_string())));
            }
            Ok(StreamEvent::Error(msg, code)) => out.push(Ok(Event::default().data(json!({
                "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
                "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
                "error": { "message": msg, "code": code },
            }).to_string()))),
            Err(e) => out.push(Ok(Event::default().data(json!({
                "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
                "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
                "error": { "message": e, "code": "stream_error" },
            }).to_string()))),
        }
        futures_util::stream::iter(out)
    });

    let done = futures_util::stream::once(async {
        Ok::<_, std::convert::Infallible>(Event::default().data("[DONE]"))
    });

    // 收尾（耗时/失败标记）由 StreamGuard 在流 drop 时统一回填，无需额外日志行
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

/// 非流式：聚合全部事件产出 chat.completion（返回 (响应, 是否有实际内容)）
async fn blocking_response(
    model: &str,
    platform: &str,
    events: EventStream,
) -> (axum::response::Response, bool) {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut usage: Option<(u64, u64, Option<u64>)> = None;
    let mut failure: Option<String> = None;
    // 工具调用聚合：index → (id, name, arguments 片段)
    let mut tool_calls: std::collections::BTreeMap<u32, (Option<String>, String, String)> = Default::default();

    let mut stream = std::pin::pin!(events);
    while let Some(ev) = stream.next().await {
        match ev {
            Ok(StreamEvent::Text(t)) => text.push_str(&t),
            Ok(StreamEvent::Reasoning(t)) => reasoning.push_str(&t),
            Ok(StreamEvent::ToolCall { index, id, name, arguments }) => {
                let entry = tool_calls.entry(index).or_default();
                if let Some(i) = id { entry.0 = Some(i); }
                if let Some(n) = name { entry.1.push_str(&n); }
                if let Some(a) = arguments { entry.2.push_str(&a); }
            }
            Ok(StreamEvent::Usage { input, output, total }) => usage = Some((input, output, total)),
            Ok(StreamEvent::Done { .. }) => {}
            Ok(StreamEvent::Error(msg, _)) => failure = Some(msg),
            Err(e) => failure = Some(e),
        }
    }
    let mut empty_stream = false;
    if let Some(msg) = failure {
        if text.is_empty() {
            return (err_response(StatusCode::BAD_GATEWAY, "upstream", &msg), false);
        }
    }
    if text.is_empty() && reasoning.is_empty() && tool_calls.is_empty() {
        empty_stream = true;
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
    let finish_reason = if tool_calls.is_empty() { "stop" } else { "tool_calls" };
    if !tool_calls.is_empty() {
        let arr: Vec<serde_json::Value> = tool_calls.into_iter().map(|(idx, (id, name, args))| {
            json!({
                "index": idx,
                "id": id.unwrap_or_else(|| format!("call_{}", uuid::Uuid::new_v4().simple())),
                "type": "function",
                "function": { "name": name, "arguments": sanitize_tool_arguments(&args) },
            })
        }).collect();
        message["tool_calls"] = json!(arr);
    }
    let resp = Json(json!({
        "id": format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()),
        "object": "chat.completion",
        "created": created,
        "model": model,
        "system_fingerprint": format!("gateway-{platform}"),
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": finish_reason,
        }],
        "usage": {
            "prompt_tokens": input,
            "completion_tokens": output,
            "total_tokens": total.unwrap_or(input + output),
        },
    }))
    .into_response();
    (resp, !empty_stream)
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


#[cfg(test)]
mod tool_args_tests {
    use super::sanitize_tool_arguments;

    fn parse(s: &str) -> serde_json::Value {
        serde_json::from_str(s).expect("修复后必须合法")
    }

    #[test]
    fn repairs_quotes_after_last_number() {
        // 实测伪影：{"limit":100""}
        let raw = r#"{"file_path":"E:\\a.md","limit":100""}"#;
        let v = parse(&sanitize_tool_arguments(raw));
        assert_eq!(v["limit"], 100);
        assert_eq!(v["file_path"], "E:\\a.md");
    }

    #[test]
    fn repairs_quotes_after_middle_number() {
        // 实测伪影：{"limit":75"","offset":50""}
        let raw = r#"{"file_path":"E:\\auto_change\\credit-rs\\crates\\credit-gateway\\src\\protocol.rs","limit":75"","offset":50""}"#;
        let v = parse(&sanitize_tool_arguments(raw));
        assert_eq!(v["limit"], 75);
        assert_eq!(v["offset"], 50);
    }

    #[test]
    fn repairs_quote_after_string_value() {
        let raw = r#"{"city":"北京""}"#;
        let v = parse(&sanitize_tool_arguments(raw));
        assert_eq!(v["city"], "北京");
    }

    #[test]
    fn keeps_valid_json_unchanged() {
        let raw = r#"{"city":"北京","temp":18}"#;
        assert_eq!(sanitize_tool_arguments(raw), raw);
    }

    #[test]
    fn keeps_legit_empty_string() {
        let raw = r#"{"query":"","limit":10}"#;
        assert_eq!(sanitize_tool_arguments(raw), raw);
    }

    #[test]
    fn empty_and_object_args() {
        assert_eq!(sanitize_tool_arguments(""), "");
        assert_eq!(sanitize_tool_arguments("{}"), "{}");
    }
}