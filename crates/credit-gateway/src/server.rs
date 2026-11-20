//! 网关 HTTP 服务：对外 OpenAI 标准接口 + 管理/账号路由接口。
//!
//! 对外（像本地 Ollama 一样只给接口地址）：
//!   GET  /v1/models                    — 模型清单（仅当前请求平台的模型）
//!   POST /v1/chat/completions          — OpenAI chat completions（stream SSE / 非流式）
//!   GET  /v1/platforms                 — 平台清单 + 当前平台
//!
//! 管理面（UI 账号路由页调用）：
//!   GET/POST /admin/rules              — 路由规则 CRUD
//!   PUT/DELETE /admin/rules/:id

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

/// 网关全局状态：Store / 路由器 / 鉴权 / 限速窗口 / 设备身份与模型目录缓存
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
    /// 模型目录缓存（当前平台；外部 Agent 频繁拉 /v1/models，避免每次打上游。
    /// 带平台键：切平台立即失效重建，避免把旧平台清单吐给新平台客户端）
    pub models_cache: std::sync::Mutex<Option<(Instant, String, serde_json::Value)>>,
    /// 各平台模型信息缓存（raw → (拉取时间, 模型id → 信息)）：长上下文截断按模型窗口计算
    pub models_info: ModelsInfoCache,
}

/// 模型信息缓存：平台 raw → (拉取时间, 模型id → 模型信息)
type ModelsInfoCache = std::sync::Mutex<std::collections::HashMap<String, (Instant, std::collections::HashMap<String, upstream::ModelInfo>)>>;
/// 工具调用聚合行：index → (id, name, 参数分片)
type ToolCallBuf = std::collections::BTreeMap<u32, (Option<String>, String, String)>;

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
    /// Bearer 鉴权校验（api_key 为空 = 放行）
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
        let out = credit_core::proc_util::hide_console(Command::new("node"))
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

/// Turing 设备 token 获取单飞锁：缓存失效时只允许一个请求去 spawn worker.cjs（最长 20s），
/// 其余并发请求等待后直接读缓存 —— 旧实现持 device_token 锁跑阻塞子进程，会卡死所有 WorkBuddy 并发请求。
fn wb_token_fetch_lock() -> &'static tokio::sync::Mutex<()> {
    static L: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    L.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Turing 设备 token（带 5min 缓存 + 单飞刷新；子进程调用放阻塞线程池，不卡 tokio worker）
async fn wb_device_token(st: &Arc<GatewayState>) -> String {
    const TTL: std::time::Duration = std::time::Duration::from_secs(300);
    {
        let cache = st.device_token.lock().unwrap();
        if let Some((t, at)) = cache.as_ref() {
            if at.elapsed() < TTL {
                return t.clone();
            }
        }
    }
    let _single = wb_token_fetch_lock().lock().await;
    // 双检：等锁期间别的请求可能已刷新
    {
        let cache = st.device_token.lock().unwrap();
        if let Some((t, at)) = cache.as_ref() {
            if at.elapsed() < TTL {
                return t.clone();
            }
        }
    }
    let fresh = tokio::task::spawn_blocking(fetch_device_token)
        .await
        .unwrap_or(None)
        .unwrap_or_default();
    if !fresh.is_empty() {
        *st.device_token.lock().unwrap() = Some((fresh.clone(), std::time::Instant::now()));
    }
    fresh
}

/// WorkBuddy 设备三元组：(device_token, machine_id, session_id)。盐派生 + token 缓存刷新。
async fn wb_device_identity(st: &Arc<GatewayState>, uid: &str) -> (String, String, String) {
    use sha2::{Digest, Sha256};
    let salt = st.device_salt.lock().unwrap().clone();
    let machine = if salt.is_empty() || uid.is_empty() { String::new() } else {
        Sha256::digest(format!("dsh-connect-workbuddy/device/v1\0{salt}\0machine\0{uid}")).iter().map(|b| format!("{b:02x}")).collect::<String>().chars().take(36).collect()
    };
    let session = if salt.is_empty() || uid.is_empty() { String::new() } else {
        Sha256::digest(format!("dsh-connect-workbuddy/device/v1\0{salt}\0session\0{uid}")).iter().map(|b| format!("{b:02x}")).collect::<String>().chars().take(36).collect()
    };
    let dev_token = wb_device_token(st).await;
    (dev_token, machine, session)
}

/// 网关自身错误响应（OpenAI error 信封，type=gateway_error）
fn err_response(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({ "error": { "message": message, "type": "gateway_error", "code": code } })),
    )
        .into_response()
}

/// 客户端请求错误响应（type=invalid_request_error）
fn openai_error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({ "error": { "message": message, "type": "invalid_request_error", "code": code } })),
    )
        .into_response()
}

/// 组装路由表：OpenAI 接口 + admin 管理面 + 健康检查（带宽松 CORS）
pub fn build_router(state: Arc<GatewayState>) -> AxumRouter {
    // 允许任意来源（本地 WebView / 第三方客户端跨端口调用）
    let cors = tower_http::cors::CorsLayer::new()
        .allow_origin(tower_http::cors::Any)
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any);
    // 请求体上限：axum `Json` extractor 默认只有 2MB —— Agent 客户端（dsh/Cline 等）
    // 长上下文 + 工具结果很容易超过 2MB，超限会在进入 handler 前被框架直接拒绝，
    // 表现为 `413 Failed to buffer the request body: length limit exceeded`。
    // 网关自身有「按模型上下文窗口截断」保护，这里放宽到 64MB。
    let body_limit = axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024);
    AxumRouter::new()
        .route("/v1/models", get(list_models))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/context-usage", get(context_usage))
        .route("/v1/platforms", get(list_platforms))
        .route("/admin/rules", get(list_rules).post(add_rule))
        .route("/admin/rules/:id", delete(delete_rule).put(update_rule))
        .layer(body_limit)
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

/// GET /v1/platforms：平台清单（各平台账号数）+ 当前平台
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
        Platform::Minimax => {
            if svc.import_account("minimax", false).await.is_ok() {
                return true;
            }
            launch_wait_and_import(st, platform, "minimax", false).await
        }
        // Trae（国内/企业/国际）、WorkBuddy、Qoder：本地登录态扫描 + refresh_token 刷新链，
        // 均失败（本地 token 也过期）→ 拉起本机客户端等它刷新后再扫
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
            if svc.refreshed_cred(&stale.provider, &stale.uid, stale).await.is_ok() {
                return true;
            }
            launch_wait_and_import(st, platform, provider, intl).await
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
            // 码道续期链：客户端会话已变 → 重新导入；客户端在跑 → 等它刷新；
            // 否则网关用 STS RT 自己续期并回写客户端 secretStorage（见 Service::ensure_codearts_fresh）
            match svc.ensure_codearts_fresh(stale).await {
                Ok(_) => true,
                Err(e) => {
                    eprintln!("[gateway] 码道凭据续期失败: {e}");
                    false
                }
            }
        }
    }
}

/// 拉起本机平台客户端（trae/workbuddy/qoder/minimax），等它自动刷新登录态
/// （凭据指纹变化）后重新扫描导入。进程拉起与轮询在线程池做，不阻塞异步运行时。
async fn launch_wait_and_import(st: &Arc<GatewayState>, platform: Platform, provider: &str, intl: bool) -> bool {
    // 1) 旧指纹（可能涉及解密/DPAPI，放线程池）
    let old_fp = tokio::task::spawn_blocking(move || match platform {
        Platform::Trae | Platform::TraeEnterprise | Platform::TraeIntl => credit_core::adapters::trae_local::token_fingerprint(),
        Platform::WorkBuddy | Platform::WorkBuddyIntl => credit_core::adapters::wb_local::session_fingerprint(intl),
        Platform::Qoder => credit_core::adapters::qoder_local::token_fingerprint(intl),
        Platform::Minimax => credit_core::adapters::minimax_local::token_fingerprint(),
        _ => None,
    }).await.ok().flatten();
    // 2) 拉起本机客户端
    let exe = match platform {
        Platform::Trae | Platform::TraeEnterprise | Platform::TraeIntl => credit_core::adapters::trae_local::launch_client(),
        Platform::WorkBuddy | Platform::WorkBuddyIntl => credit_core::adapters::wb_local::launch_client(),
        Platform::Qoder => credit_core::adapters::qoder_local::launch_client(),
        Platform::Minimax => credit_core::adapters::minimax_local::launch_client(),
        _ => None,
    };
    match exe {
        Some(exe) => eprintln!("[gateway] {provider} 凭据过期，已拉起本机客户端 {}，等待其刷新…", exe.display()),
        None => {
            eprintln!("[gateway] {provider} 凭据过期且本机未安装客户端");
            return false;
        }
    }
    // 3) 等客户端刷新凭据（最多 15s）
    let ok = tokio::task::spawn_blocking(move || match platform {
        Platform::Trae | Platform::TraeEnterprise | Platform::TraeIntl => {
            credit_core::adapters::trae_local::wait_for_token_change(old_fp.as_deref(), std::time::Duration::from_secs(15)).is_some()
        }
        Platform::WorkBuddy | Platform::WorkBuddyIntl => {
            credit_core::adapters::wb_local::wait_for_session_change(intl, old_fp.as_deref(), std::time::Duration::from_secs(15)).is_some()
        }
        Platform::Qoder => {
            credit_core::adapters::qoder_local::wait_for_token_change(intl, old_fp.as_deref(), std::time::Duration::from_secs(15)).is_some()
        }
        Platform::Minimax => {
            credit_core::adapters::minimax_local::wait_for_token_change(old_fp.as_deref(), std::time::Duration::from_secs(15)).is_some()
        }
        _ => false,
    }).await.unwrap_or(false);
    if !ok {
        eprintln!("[gateway] {provider} 客户端 15s 内未刷新凭据（可能需要在客户端里重新登录）");
        return false;
    }
    // 4) 重新扫描导入
    use credit_core::service::Service;
    let svc = Service { store: st.store.clone() };
    svc.import_account(provider, intl).await.is_ok()
}

/// 拉取一个平台的模型列表（含码道双通道兜底）
async fn fetch_models_for(st: &Arc<GatewayState>, platform: Platform, raw: &str) -> Option<Vec<upstream::ModelInfo>> {
    let all = st.router.accounts_for(platform);
    // 码道通道偏好：raw=codearts-desktop 用桌面凭据、raw=codearts 用 IDE 凭据，
    // 另一通道作为兜底（同账号共用 STS 会话）。旧实现偏好方向写反了，
    // 导致 IDE 通道永远优先用桌面凭据、失败兜底也选反。
    let want_desktop = raw == "codearts-desktop";
    let (pool, fallback): (Vec<Cred>, Vec<Cred>) = if platform == Platform::CodeArts {
        (
            all.iter().filter(|c| (c.provider == "codearts-desktop") == want_desktop).cloned().collect(),
            all.iter().filter(|c| (c.provider == "codearts-desktop") != want_desktop).cloned().collect(),
        )
    } else {
        (all.clone(), Vec::new())
    };
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
                    if let Ok(nc) = refresh_creds_for(st, platform, &cred).await {
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
            if let Ok(c) = svc.refreshed_cred(&stale.provider, &stale.uid, stale).await {
                return Ok(c);
            }
            // 全部失败 → 拉起本机客户端等刷新
            if refresh_creds_from_local(st, platform, stale).await {
                st.router.accounts_for(platform).into_iter().next().ok_or(())
            } else {
                Err(())
            }
        }
        Platform::WorkBuddy | Platform::WorkBuddyIntl => {
            let intl = platform == Platform::WorkBuddyIntl;
            if svc.import_account("workbuddy", intl).await.is_ok() {
                if let Some(c) = st.router.accounts_for(platform).into_iter().find(|c| c.token != stale.token) {
                    return Ok(c);
                }
            }
            if let Ok(c) = svc.refreshed_cred(&stale.provider, &stale.uid, stale).await {
                return Ok(c);
            }
            if refresh_creds_from_local(st, platform, stale).await {
                st.router.accounts_for(platform).into_iter().next().ok_or(())
            } else {
                Err(())
            }
        }
        Platform::Qoder => {
            if svc.import_account("qoder", false).await.is_ok() {
                if let Some(c) = st.router.accounts_for(platform).into_iter().find(|c| c.token != stale.token) {
                    return Ok(c);
                }
            }
            if let Ok(c) = svc.refreshed_cred(&stale.provider, &stale.uid, stale).await {
                return Ok(c);
            }
            if refresh_creds_from_local(st, platform, stale).await {
                st.router.accounts_for(platform).into_iter().next().ok_or(())
            } else {
                Err(())
            }
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

/// 构建当前请求平台的模型目录（严格跟随账号路由页「请求平台」下拉框，
/// 只返回该平台的模型 —— 不再聚合其它平台）。
async fn build_models_catalog(st: &Arc<GatewayState>, current_raw: &str) -> serde_json::Value {
    let mut data = Vec::new();
    if let Some(platform) = Platform::parse(current_raw) {
        if let Some(models) = fetch_models_for(st, platform, current_raw).await {
            for m in models {
                data.push(json!({
                    "id": m.id.clone(),
                    "object": "model",
                    "created": 0,
                    "owned_by": current_raw,
                    "platform": current_raw,
                    "raw_model": m.id,
                    "name": m.name,
                    "context_window": m.context_window,
                    "max_tokens": m.max_tokens,
                    "credit_multiplier": m.credit_multiplier,
                }));
            }
        }
    }
    json!({ "object": "list", "data": data })
}

/// GET /v1/context-usage: context consumption snapshot for agent-side compaction.
/// Agent asks this before each turn: usageRatio >= compactThreshold -> compact now.
async fn context_usage(
    State(st): State<Arc<GatewayState>>,
    headers: axum::http::HeaderMap,
) -> Response {
    if !st.check_auth(headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok())) {
        return openai_error_response(StatusCode::UNAUTHORIZED, "unauthorized", "missing or invalid Authorization bearer");
    }
    // 模型提示：取最近一条网关日志的模型（没有则为空 → 用默认窗口）
    let model_hint = st.store.list_gateway_logs(1).ok().and_then(|l| l.first().map(|x| x.model.clone())).unwrap_or_default();
    let (platform, model) = resolve_model(&model_hint, st.router.current_platform())
        .unwrap_or((st.router.current_platform(), model_hint));
    let raw = st.router.current_platform_raw();
    let (context_window, _mt) = if model.is_empty() {
        (DEFAULT_CONTEXT_WINDOW, None)
    } else {
        model_limits(&st, platform, &raw, &model).await
    };
    let rows = st.store.token_stats_daily(1).unwrap_or_default();
    let (pt, ct, total, reqs) = rows.first().map(|r| (r.1, r.2, r.3, r.4)).unwrap_or((0, 0, 0, 0));
    let ratio = if context_window > 0 { (total as f64 / context_window as f64).min(1.0) } else { 0.0 };
    Json(json!({
        "model": model,
        "platform": platform.as_str(),
        "contextWindow": context_window,
        "usedToday": { "promptTokens": pt, "completionTokens": ct, "totalTokens": total, "requests": reqs },
        "estimatedUsed": total,
        "usageRatio": (ratio * 10000.0).round() / 10000.0,
        "shouldCompact": ratio >= 0.85,
        "compactThreshold": 0.85,
        "policy": "agent_decides: gateway publishes usage; agent summarizes and trims history when shouldCompact=true"
    })).into_response()
}

/// GET /v1/models：当前请求平台的模型目录（60s 缓存，切平台立即重建）
async fn list_models(
    State(st): State<Arc<GatewayState>>,
    headers: axum::http::HeaderMap,
) -> Response {
    if !st.check_auth(headers.get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok())) {
        return openai_error_response(StatusCode::UNAUTHORIZED, "unauthorized", "missing or invalid Authorization bearer");
    }
    // 60s 缓存（外部 Agent 会频繁拉模型列表；上游拉取含网络请求与 sidecar spawn）。
    // 缓存带平台键：切换请求平台后立即重建，不会把旧平台清单吐给新平台客户端。
    let current_raw = st.router.current_platform_raw();
    if let Ok(g) = st.models_cache.lock() {
        if let Some((at, plat, cached)) = &*g {
            if at.elapsed() < std::time::Duration::from_secs(60) && plat == &current_raw {
                return Json(cached.clone()).into_response();
            }
        }
    }
    let catalog = build_models_catalog(&st, &current_raw).await;
    if let Ok(mut g) = st.models_cache.lock() {
        *g = Some((Instant::now(), current_raw, catalog.clone()));
    }
    Json(catalog).into_response()
}

// ---------- 长上下文截断 ----------

/// 模型信息缓存 TTL
const MODEL_INFO_TTL: std::time::Duration = std::time::Duration::from_secs(600);
/// 目录缺失/拉取失败时的兜底上下文窗口（平台静态目录大多 ≤128k）
const DEFAULT_CONTEXT_WINDOW: u64 = 128_000;

/// 平台模型目录（id → 模型信息；10 分钟缓存；拉取失败返回 None，不阻塞请求）。
/// 供长上下文截断、模型归属校验与跨平台自动寻址共用。
async fn platform_catalog(
    st: &Arc<GatewayState>,
    platform: Platform,
    raw: &str,
) -> Option<std::collections::HashMap<String, upstream::ModelInfo>> {
    {
        let map = st.models_info.lock().unwrap();
        if let Some((at, m)) = map.get(raw) {
            if at.elapsed() < MODEL_INFO_TTL {
                return Some(m.clone());
            }
        }
    }
    let models = fetch_models_for(st, platform, raw).await?;
    let mut m = std::collections::HashMap::new();
    for info in models {
        m.insert(info.id.clone(), info);
    }
    if let Ok(mut map) = st.models_info.lock() {
        map.insert(raw.to_string(), (Instant::now(), m.clone()));
    }
    Some(m)
}

/// 查询模型的上下文窗口与输出上限（带 10min 平台级缓存；拉取失败退回默认值）
async fn model_limits(st: &Arc<GatewayState>, platform: Platform, raw: &str, model: &str) -> (u64, Option<u64>) {
    match platform_catalog(st, platform, raw).await {
        Some(map) => match map.get(model) {
            Some(info) => (info.context_window.unwrap_or(DEFAULT_CONTEXT_WINDOW), info.max_tokens),
            None => (DEFAULT_CONTEXT_WINDOW, None),
        },
        None => (DEFAULT_CONTEXT_WINDOW, None),
    }
}

/// 保守 token 估算：UTF-8 字节数 / 3 + 少量结构开销。
/// 中文约 3 字节 ≈ 1~2 token（不低估）；英文约 4 字符 ≈ 1 token（略高估）。
fn estimate_tokens(s: &str) -> u64 {
    s.len() as u64 / 3 + 8
}

/// JSON 值的 token 估算（序列化后按字节估）
fn estimate_tokens_value(v: &Value) -> u64 {
    estimate_tokens(&v.to_string())
}

/// 取消息 role（缺省空串）
fn msg_role(m: &Value) -> &str {
    m.get("role").and_then(|r| r.as_str()).unwrap_or("")
}

/// 值内超长字符串硬截断（兜底：防止单条超大消息打爆上下文窗口）
fn truncate_value_strings(v: &mut Value, max_chars: usize) {
    match v {
        Value::String(s) => {
            if s.chars().count() > max_chars {
                let cut: String = s.chars().take(max_chars).collect();
                *s = format!("{cut}…[truncated]");
            }
        }
        Value::Array(arr) => {
            for x in arr.iter_mut() {
                truncate_value_strings(x, max_chars);
            }
        }
        Value::Object(o) => {
            for (_k, x) in o.iter_mut() {
                truncate_value_strings(x, max_chars);
            }
        }
        _ => {}
    }
}

/// 按模型上下文窗口截断 OpenAI 消息（返回丢弃的消息条数）：
/// ① 从最早的非 system 消息开始丢弃，直到装入预算；
/// ② 截断点对齐「轮次边界」：保留部分必须从 user/system 开始，
///    保证 assistant.tool_calls 与其 tool 回复成对保留（孤儿 tool 回复会被上游拒绝）；
/// ③ 仍超限 → 按比例硬截断超长 system 文本（保留头部）；
/// ④ 最后兜底：截断残余的超长字符串内容。
fn truncate_openai_messages(messages: &mut Vec<Value>, tools: &[Value], context_window: u64, max_tokens_req: Option<u32>) -> usize {
    let safety = 1024u64;
    let reserve = max_tokens_req.map(|t| t as u64)
        .unwrap_or_else(|| (context_window / 4).clamp(1024, 8192));
    let budget = context_window.saturating_sub(reserve).saturating_sub(safety);
    if budget == 0 {
        return 0;
    }
    let est = |msgs: &[Value]| -> u64 {
        tools.iter().map(estimate_tokens_value).sum::<u64>()
            + msgs.iter().map(estimate_tokens_value).sum::<u64>()
    };
    let mut used = est(messages);
    if used <= budget {
        return 0;
    }
    let mut dropped = 0usize;
    // 头部 system 保留；从第一条非 system 消息开始丢最老的（至少保留最后 1 条）
    let sys_head = messages.iter().take_while(|m| msg_role(m) == "system").count();
    while messages.len() > sys_head + 1 && used > budget {
        used -= estimate_tokens_value(&messages[sys_head]);
        messages.remove(sys_head);
        dropped += 1;
    }
    // 对齐轮次边界（不丢头部 system，不动最后 1 条）
    while messages.len() > sys_head + 1 && msg_role(&messages[sys_head]) != "user" {
        used -= estimate_tokens_value(&messages[sys_head]);
        messages.remove(sys_head);
        dropped += 1;
    }
    // 仍超限 → 逐条硬截断超长 system 文本（按 预算/占用 比例保留头部）
    if used > budget {
        for m in messages.iter_mut() {
            if msg_role(m) != "system" {
                continue;
            }
            let Some(s) = m.get("content").and_then(|c| c.as_str()).map(|s| s.to_string()) else { continue };
            let tokens = estimate_tokens(&s);
            if tokens <= 512 {
                continue;
            }
            let keep_ratio = (budget as f64 / used.max(1) as f64).clamp(0.05, 1.0);
            let keep_chars = ((s.chars().count() as f64) * keep_ratio) as usize;
            let cut: String = s.chars().take(keep_chars).collect::<String>() + "\n…[system truncated]";
            m["content"] = json!(cut);
            used = used.saturating_sub(tokens) + estimate_tokens(&cut);
            if used <= budget {
                break;
            }
        }
    }
    // 最后兜底：截断残余超长字符串
    if used > budget {
        if let Some(last) = messages.last_mut() {
            let max_chars = (budget.saturating_mul(3) as usize).max(4096);
            truncate_value_strings(last, max_chars);
        }
    }
    dropped
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

/// POST /v1/chat/completions 主入口：鉴权 → 限速 → 解析模型/平台 → 路由规则 →
/// 长上下文截断 → 逐账号 failover 派发（含凭据自愈与 11128 渠道退避重试）
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
    let mut openai_messages = body.get("messages").and_then(|m| m.as_array()).cloned().unwrap_or_default();
    if openai_messages.is_empty() {
        return openai_error_response(StatusCode::BAD_REQUEST, "missing_messages", "field \"messages\" is required");
    }
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
    let current_raw = st.router.current_platform_raw();
    let mut raw_for_models = if platform == current { current_raw.clone() } else { platform.as_str().to_string() };
    // 模型校验 + 跨平台自动寻址（strict=off 且未固定账号时）：
    // 客户端（如 dsh/Cherry）常用裸模型 id，切换请求平台后模型可能不属于新平台。
    // 旧行为是把请求原样打给新平台 → 上游 400/空流/502；现在按各平台模型目录找到真正
    // 拥有该模型的平台并自动路由；都找不到则返回明确的 400 model_not_found + 可用模型。
    if !strict && pinned_account.is_none() {
        if let Some(cat) = platform_catalog(&st, platform, &raw_for_models).await {
            if !cat.contains_key(&model) {
                let mut owner: Option<(Platform, String)> = None;
                for p in Platform::all() {
                    if p == platform || !p.chat_capable() {
                        continue;
                    }
                    if st.router.accounts_for(p).is_empty() {
                        continue;
                    }
                    let raw_p = if p == current { current_raw.clone() } else { p.as_str().to_string() };
                    if let Some(cat2) = platform_catalog(&st, p, &raw_p).await {
                        if cat2.contains_key(&model) {
                            owner = Some((p, raw_p));
                            break;
                        }
                    }
                }
                if let Some((p, raw_p)) = owner {
                    eprintln!("[gateway] 模型 {} 不属于 {}，按平台目录自动路由到 {}", model, platform.as_str(), p.as_str());
                    platform = p;
                    raw_for_models = raw_p;
                } else {
                    let mut avail: Vec<String> = cat.keys().cloned().collect();
                    avail.sort();
                    avail.truncate(12);
                    let _ = st.store.log_gateway_request(platform.as_str(), "", &model, false, Some(400), Some("model_not_found"), None);
                    return openai_error_response(
                        StatusCode::BAD_REQUEST,
                        "model_not_found",
                        &format!(
                            "模型 \"{model}\" 不存在于平台 {}。可用模型：{}；也可用 \"平台/模型\" 指定其它平台（如 minimax/MiniMax-M3.1-Flash）",
                            platform.display(),
                            avail.join(", ")
                        ),
                    );
                }
            }
        }
    }
    let creds: Vec<Cred> = match pinned_account {
        Some(c) => vec![c],
        None => st.router.accounts_for(platform),
    };
    if creds.is_empty() {
        return err_response(StatusCode::SERVICE_UNAVAILABLE, "no_account",
            &format!("平台 {} 没有可用账号，请先在账号管理添加或扫描导入", platform.display()));
    }

    // 长上下文保护：按所选模型的上下文窗口截断请求（Agent 累积历史/超大 system 会在上游
    // 挂死或长时间无响应）。截断在 normalize 之前做，保留 assistant.tool_calls 与 tool 回复的配对。
    let (context_window, model_max_tokens) = model_limits(&st, platform, &raw_for_models, &model).await;
    let dropped = truncate_openai_messages(&mut openai_messages, &tools, context_window, max_tokens);
    if dropped > 0 {
        eprintln!("[gateway] 长上下文截断: 模型 {model} 窗口 {context_window}，丢弃 {dropped} 条最早消息");
    }
    let (messages, system) = normalize_messages(&openai_messages);
    // max_tokens 封顶：不超过模型上限，且输入估算 + 输出 ≤ 上下文窗口
    let max_tokens = max_tokens.map(|mt| {
        let mut v = mt;
        if let Some(mm) = model_max_tokens { v = v.min(mm.min(u32::MAX as u64) as u32); }
        let room = context_window.saturating_sub(
            openai_messages.iter().map(estimate_tokens_value).sum::<u64>()
            + tools.iter().map(estimate_tokens_value).sum::<u64>()
        ).max(1024) as u32;
        v.min(room)
    });

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
                        let log_id = st.store.log_gateway_request_full(
                            platform.as_str(), &account_ref, &model, true, Some(200), None, None, None, None,
                        ).unwrap_or(0);
                        stream_response(&st, &model, platform.as_str(), events, log_id, t0)
                    } else {
                        // 非流式：聚合完成后再按实际内容记录（空流记失败，避免"假成功"）
                        let (resp, has_content, usage) = blocking_response(&model, platform.as_str(), events).await;
                        let (pt, ct) = usage.map(|(p, c, _)| (Some(p as i64), Some(c as i64))).unwrap_or((None, None));
                        let log_id = st.store.log_gateway_request_full(
                            platform.as_str(),
                            &format!("{}:{}", attempt_cred.provider, attempt_cred.uid),
                            &model, has_content, Some(if has_content { 200 } else { 502 }),
                            if has_content { None } else { Some("empty stream (no finish_reason/content)") },
                            None, pt, ct,
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
            // 设备身份（token 单飞刷新，不跨 await 持 std 锁）
            let (dev_token, machine, session) = wb_device_identity(st, &cred.uid).await;
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
            // 传 OpenAI 线格式 body（zcode_chat_stream 内部转换 Anthropic + profile 取票）
            let mut body = serde_json::json!({
                "model": model,
                "messages": messages,
                "system": system,
                "stream": true,
            });
            if let Some(mt) = max_tokens { body["max_tokens"] = serde_json::json!(mt); }
            if let Some(re) = reasoning_effort { body["reasoning_effort"] = serde_json::json!(re); }
            if !tools.is_empty() {
                body["tools"] = serde_json::json!(tools);
                if let Some(tc) = tool_choice { body["tool_choice"] = tc.clone(); }
            }
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
            let mut body = crate::protocol::codearts_chat_body(messages, system, model, max_tokens, reasoning_effort);
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
    tokens: Arc<std::sync::Mutex<(Option<i64>, Option<i64>)>>,
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        let elapsed = self.t0.elapsed().as_millis() as i64;
        let finished = self.finished.load(std::sync::atomic::Ordering::Relaxed);
        if finished {
            let (pt, ct) = *self.tokens.lock().unwrap_or_else(|e| e.into_inner());
            if pt.is_some() || ct.is_some() {
                let _ = self.store.update_gateway_log_tokens(self.log_id, pt, ct);
            }
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
    let tokens_seen: Arc<std::sync::Mutex<(Option<i64>, Option<i64>)>> = Arc::default();
    let tokens_seen2 = tokens_seen.clone();
    let guard = StreamGuard {
        store: st.store.clone(),
        log_id,
        t0,
        finished,
        error,
        tokens: tokens_seen,
    };

    // 工具参数按 index 缓冲：上游分片可能含伪影（如 Qoder 多发 `""`），
    // 直接透传会让客户端拼出非法 JSON；这里缓存分片，在结束事件时校验/修复后一次性下发。
    // 共享缓冲 + 一次性下发标记：上游没发结束帧就断开（长任务常见）时，
    // 收尾流仍会补发已缓冲的参数，避免 Agent 收到没有参数的 tool call 后挂死。
    let tool_buf: Arc<std::sync::Mutex<ToolCallBuf>> = Arc::default();
    let tool_buf2 = tool_buf.clone();
    let tool_flushed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let tool_flushed2 = tool_flushed.clone();
    // 结束帧只发一次：部分上游（码道/部分 OpenAI 兼容网关）会在同一响应里重复 finish_reason，
    // 直接透传会让客户端看到两帧 finish_reason（有的客户端会提前截断/报错）
    let done_sent = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done_sent2 = done_sent.clone();
    // 收尾流要用 id/model，但 flat_map 的 move 闭包会拿走原值 → 先克隆
    let id_c = id.clone();
    let model_c = model.clone();

    let body_stream = events.flat_map(move |ev| {
        // guard 随闭包存活 → 流结束/中断时 Drop 回填耗时
        let _keep_alive = &guard;
        let mut out: Vec<Result<Event, std::convert::Infallible>> = Vec::new();
        match &ev {
            Ok(StreamEvent::Done { .. }) => finished2.store(true, std::sync::atomic::Ordering::Relaxed),
            Ok(StreamEvent::ToolCall { .. }) => tool_seen2.store(true, std::sync::atomic::Ordering::Relaxed),
            Ok(StreamEvent::Error(m, _)) | Err(m) => {
                if let Ok(mut g) = error2.lock() {
                    if g.is_none() {
                        *g = Some(m.clone());
                    }
                }
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
                let mut g = tool_buf2.lock().unwrap();
                let e = g.entry(index).or_default();
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
            Ok(StreamEvent::Usage { input, output, total }) => {
                if let Ok(mut g) = tokens_seen2.lock() {
                    *g = (Some(input as i64), Some(output as i64));
                }
                out.push(Ok(Event::default().data(json!({
                "id": id, "object": "chat.completion.chunk", "created": created, "model": model,
                "choices": [],
                "usage": {
                    "prompt_tokens": input,
                    "completion_tokens": output,
                    "total_tokens": total.unwrap_or(input + output),
                },
            }).to_string())));
            }
            Ok(StreamEvent::Done { finish }) => {
                // 重复 finish_reason：只处理第一帧（含参数 flush 与结束帧下发）
                if done_sent2.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    return futures_util::stream::iter(out);
                }
                if !tool_flushed2.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    for (index, (_tid, _name, args)) in tool_buf2.lock().unwrap().iter() {
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

    // 收尾流：上游未发结束帧就断开时（网络中断/上游非标准关闭），这里补发缓冲的
    // 工具参数与 finish_reason，保证 Agent 拿到完整 tool_calls 继续任务而不是挂死。
    // 正常收到 Done 时 tool_flushed 已置位，本流输出为空，不影响正常路径。
    //
    // 注意：必须**惰性**构造（once(async).flatten()）。此前用 stream::iter({...}) 在
    // stream_response 返回时就立即执行了闭包：先把 tool_flushed 置位、把 finish_reason
    // 按「此刻」的空状态算成 stop，导致①每个响应多一帧 finish_reason，②正常结束路径的
    // 工具参数永远不 flush（Agent 收到没有参数的 tool_calls 而挂死）。
    let close = futures_util::stream::once(async move {
        let mut out: Vec<Result<Event, std::convert::Infallible>> = Vec::new();
        if !tool_flushed.swap(true, std::sync::atomic::Ordering::Relaxed) {
            for (index, (_tid, _name, args)) in tool_buf.lock().unwrap().iter() {
                let args_str = if args.trim().is_empty() {
                    "{}".to_string()
                } else {
                    sanitize_tool_arguments(args)
                };
                let tc = json!({ "index": index, "function": { "arguments": args_str } });
                out.push(Ok(Event::default().data(json!({
                    "id": id_c, "object": "chat.completion.chunk", "created": created, "model": model_c,
                    "choices": [{ "index": 0, "delta": { "tool_calls": [tc] }, "finish_reason": null }],
                }).to_string())));
            }
            let fr = if tool_seen.load(std::sync::atomic::Ordering::Relaxed) { "tool_calls" } else { "stop" };
            out.push(Ok(Event::default().data(json!({
                "id": id_c, "object": "chat.completion.chunk", "created": created, "model": model_c,
                "choices": [{ "index": 0, "delta": {}, "finish_reason": fr }],
            }).to_string())));
        }
        futures_util::stream::iter(out)
    })
    .flatten();

    let done = futures_util::stream::once(async {
        Ok::<_, std::convert::Infallible>(Event::default().data("[DONE]"))
    });

    // 收尾（耗时/失败标记）由 StreamGuard 在流 drop 时统一回填，无需额外日志行
    Sse::new(futures_util::stream::iter([Ok::<_, std::convert::Infallible>(prelude)])
        .chain(body_stream)
        .chain(close)
        .chain(done))
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// 归一化上游 finish_reason → OpenAI 语义（stop / length / tool_calls）
fn normalize_finish(finish: &str) -> String {
    match finish {
        "tool-calls" | "tool_calls" => "tool_calls".into(),
        "max-tokens" | "length" => "length".into(),
        _ => "stop".into(),
    }
}

/// 非流式：聚合全部事件产出 chat.completion（返回 (响应, 是否有实际内容)）
async fn blocking_response(
    model: &str,
    platform: &str,
    events: EventStream,
) -> (axum::response::Response, bool, Option<(u64, u64, Option<u64>)>) {
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
            Ok(StreamEvent::Error(msg, _)) => {
                // 只保留第一个错误（最接近根因；后续多为上游栈帧噪声）
                if failure.is_none() {
                    failure = Some(msg);
                }
            }
            Err(e) => {
                if failure.is_none() {
                    failure = Some(e);
                }
            }
        }
    }
    let mut empty_stream = false;
    if let Some(msg) = failure {
        if text.is_empty() {
            return (err_response(StatusCode::BAD_GATEWAY, "upstream", &msg), false, usage);
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
    (resp, !empty_stream, usage)
}

// ---------- 管理面 ----------

/// GET /admin/rules：路由规则清单
async fn list_rules(State(st): State<Arc<GatewayState>>) -> Response {
    Json(json!({ "rules": st.router.list_rules() })).into_response()
}

/// POST /admin/rules：新增路由规则
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

/// PUT /admin/rules/:id：更新路由规则（不存在的字段回落原值）
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

/// DELETE /admin/rules/:id：删除路由规则
async fn delete_rule(State(st): State<Arc<GatewayState>>, axum::extract::Path(id): axum::extract::Path<String>) -> Response {
    if st.router.delete_rule(&id) {
        Json(json!({ "ok": true })).into_response()
    } else {
        err_response(StatusCode::NOT_FOUND, "not_found", "rule not found")
    }
}


#[cfg(test)]
mod truncate_tests {
    use super::*;

    fn user_msg(s: &str) -> Value {
        json!({ "role": "user", "content": s })
    }

    #[test]
    fn keeps_short_context_untouched() {
        let mut msgs = vec![user_msg("你好"), user_msg("再见")];
        assert_eq!(truncate_openai_messages(&mut msgs, &[], 128_000, None), 0);
        assert_eq!(msgs.len(), 2);
    }

    #[test]
    fn drops_oldest_messages_when_over_budget() {
        // 每条 ~34k token（102k 字节），窗口 128k，预算约 121k → 4 条会超
        let big = "x".repeat(102_000);
        let mut msgs: Vec<Value> = (0..4).map(|_| user_msg(&big)).collect();
        let dropped = truncate_openai_messages(&mut msgs, &[], 128_000, None);
        assert!(dropped > 0, "应丢弃超预算的旧消息");
        let est: u64 = msgs.iter().map(estimate_tokens_value).sum::<u64>();
        let reserve = 8192u64;
        assert!(est <= 128_000 - reserve - 1024, "截断后必须装入预算: est={est}");
    }

    #[test]
    fn truncation_aligns_to_user_boundary() {
        // user(超大) + assistant(tool_calls) + tool + user 序列：
        // 丢弃最老消息后落点在 assistant/tool 中间，必须继续丢到 user 轮次边界
        let big = "y".repeat(402_000); // ~134k token > 预算 118k
        let mut msgs = vec![
            user_msg(&big),
            json!({ "role": "assistant", "content": "ok", "tool_calls": [{ "id": "1", "function": { "name": "f", "arguments": "{}" } }] }),
            json!({ "role": "tool", "tool_call_id": "1", "content": "result" }),
            user_msg("最新问题"),
        ];
        let dropped = truncate_openai_messages(&mut msgs, &[], 128_000, None);
        assert!(dropped > 0);
        let first_role = msg_role(&msgs[0]);
        assert!(first_role == "user" || first_role == "system",
            "保留部分必须从 user/system 轮次边界开始，实际: {first_role}");
    }

    #[test]
    fn keeps_leading_system_messages() {
        let big = "z".repeat(102_000);
        let mut msgs = vec![
            json!({ "role": "system", "content": "你是助手" }),
            user_msg(&big),
            user_msg(&big),
            user_msg("最后问题"),
        ];
        truncate_openai_messages(&mut msgs, &[], 128_000, None);
        assert_eq!(msg_role(&msgs[0]), "system", "头部 system 必须保留");
        assert!(msgs.iter().any(|m| msg_role(m) == "user" && m["content"] == "最后问题"), "最新用户消息必须保留");
    }

    #[test]
    fn huge_system_gets_hard_truncated() {
        // system 本身超过窗口 → 允许保留但被截断标记
        let huge = "s".repeat(200 * 1024); // ~68k token
        let mut msgs = vec![
            json!({ "role": "system", "content": huge }),
            user_msg("问"),
        ];
        let dropped = truncate_openai_messages(&mut msgs, &[], 8_000, None);
        assert_eq!(dropped, 0);
        let sys = msgs[0]["content"].as_str().unwrap();
        assert!(sys.contains("[system truncated]"), "超长 system 应被截断");
    }
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