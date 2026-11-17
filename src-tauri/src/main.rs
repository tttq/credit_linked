#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Tauri 壳：rpc 命令面（对齐原版 credit:invoke 直连平台部分）。
//! snapshot / import / checkin / quota_refresh / history / login.start / login.poll / login.cancel / account.delete / account.toggle

use credit_core::db::Store;
use credit_core::service::Service;
use credit_gateway::protocol::Platform;
use credit_gateway::Router;
use std::net::SocketAddr;
use std::sync::Arc;
use tauri::Manager;

fn svc() -> Result<Service, String> {
    Ok(Service { store: Arc::new(Store::open_default().map_err(|e| e.to_string())?) })
}

#[tauri::command]
async fn snapshot() -> Result<serde_json::Value, String> {
    let s = svc()?;
    Ok(s.ui_snapshot())
}

#[tauri::command]
async fn import(platform: String, intl: bool) -> Result<serde_json::Value, String> {
    let s = svc()?;
    s.import_account(&platform, intl).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn checkin(provider: String, uid: String, all: bool) -> Result<serde_json::Value, String> {
    let s = svc()?;
    let ids: Vec<(String, String)> = if all {
        s.store.list_accounts().map_err(|e| e.to_string())?
            .into_iter().map(|(p, u, _, _, _)| (p, u)).collect()
    } else {
        vec![(provider, uid)]
    };
    let mut out = Vec::new();
    for (p, u) in ids {
        match s.checkin_account(&p, &u).await {
            Ok(o) => out.push(serde_json::json!({
                "provider": o.provider, "uid": o.uid, "name": p_name(&s, &p, &u),
                "status": o.status, "amount": o.amount, "message": o.message,
            })),
            Err(e) => out.push(serde_json::json!({
                "provider": p, "uid": u, "status": "failed", "message": e.to_string(),
            })),
        }
    }
    Ok(serde_json::json!({ "results": out }))
}

#[tauri::command]
async fn quota_refresh(provider: String, uid: String) -> Result<serde_json::Value, String> {
    let s = svc()?;
    let q = s.quota_account(&provider, &uid).await.map_err(|e| e.to_string())?;
    serde_json::to_value(q).map_err(|e| e.to_string())
}

#[tauri::command]
async fn history(limit: Option<i64>) -> Result<serde_json::Value, String> {
    let s = svc()?;
    let rows = s.store.list_history(limit.unwrap_or(50)).map_err(|e| e.to_string())?;
    serde_json::to_value(rows).map_err(|e| e.to_string())
}

// ===== 登录 =====

#[tauri::command]
async fn login_start(provider: String) -> Result<serde_json::Value, String> {
    let (sid, url, exp) = credit_core::login::start_login(&provider).await.map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "sessionId": sid, "url": url, "expiresIn": exp }))
}

#[tauri::command]
async fn login_poll(session_id: String) -> Result<serde_json::Value, String> {
    let out = credit_core::login::poll_login(&session_id).await.map_err(|e| e.to_string())?;
    let mut v = serde_json::json!({ "status": out.status });
    if let Some(c) = &out.credential {
        // 入库（多账号：同 provider+uid 覆盖）
        let s = svc()?;
        let auth = serde_json::to_string(c).map_err(|e| e.to_string())?;
        s.store.upsert_account(&c.provider, &c.uid, &c.name, "", &auth).map_err(|e| e.to_string())?;
        v["account"] = serde_json::json!({ "provider": c.provider, "uid": c.uid, "name": c.name });
    }
    Ok(v)
}

#[tauri::command]
async fn login_cancel() -> Result<(), String> {
    credit_core::login::cancel_all();
    Ok(())
}

// ===== 账号管理 =====

#[tauri::command]
async fn account_delete(provider: String, uid: String) -> Result<(), String> {
    let s = svc()?;
    s.store.delete_account(&provider, &uid).map_err(|e| e.to_string())
}

#[tauri::command]
async fn account_toggle_auto(provider: String, uid: String, auto: bool) -> Result<(), String> {
    let s = svc()?;
    s.store.set_account_auto(&provider, &uid, auto).map_err(|e| e.to_string())
}

fn p_name(s: &Service, p: &str, u: &str) -> String {
    s.store.list_accounts().ok().and_then(|accs| {
        accs.into_iter().find(|(ap, au, _, _, _)| ap == p && au == u).map(|(_, _, n, _, _)| n)
    }).unwrap_or_default()
}

#[tauri::command]
async fn statistics(provider: Option<String>, uid: Option<String>, period: Option<String>) -> Result<serde_json::Value, String> {
    let s = svc()?;
    Ok(s.statistics(provider.as_deref(), uid.as_deref(), period.unwrap_or_else(|| "day".into()).as_str()))
}

#[tauri::command]
async fn daily_income(days: Option<i64>) -> Result<serde_json::Value, String> {
    let s = svc()?;
    Ok(s.daily_income(days.unwrap_or(7)))
}

#[tauri::command]
async fn settings_get() -> Result<serde_json::Value, String> {
    let s = svc()?;
    let get = |k: &str, d: &str| s.store.get_setting(k, d).unwrap_or_else(|_| d.to_string());
    Ok(serde_json::json!({
        "auto": get("auto", "1") == "1",
        "catchup": get("catchup", "1") == "1",
        "refreshMinutes": get("refreshMinutes", "60").parse::<i64>().unwrap_or(60),
        "launchAtLogin": get("launchAtLogin", "0") == "1",
        "telemetry": get("telemetry", "0") == "1",
        "randomTime": get("randomTime", "0") == "1",
        "time": get("time", "09:00"),
        "app": "AI路由助手",
        "schedTick": get("sched_tick", ""),
        "lastRunDay": get("last_auto_run_day", ""),
        "lastRunTarget": get("last_auto_run_target", ""),
        "lastAutoError": get("last_auto_error", ""),
    }))
}

#[tauri::command]
async fn settings_set(key: String, value: String) -> Result<(), String> {
    let s = svc()?;
    s.store.set_setting(&key, &value).map_err(|e| e.to_string())
}

#[tauri::command]
async fn account_list_all() -> Result<serde_json::Value, String> {
    let s = svc()?;
    let rows = s.store.list_accounts_full().map_err(|e| e.to_string())?;
    serde_json::to_value(rows).map_err(|e| e.to_string())
}

#[tauri::command]
async fn open_url(url: String) -> Result<(), String> {
    // 跨平台用系统默认浏览器打开授权页
    #[cfg(windows)]
    std::process::Command::new("cmd").args(["/C", "start", "", &url]).spawn().map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(&url).spawn().map_err(|e| e.to_string())?;
    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open").arg(&url).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

// ===== 应用内授权子窗口（iframe 会被平台 X-Frame-Options 拒绝，改用 Tauri 子窗口内嵌授权） =====

#[tauri::command]
async fn login_open_window(app: tauri::AppHandle, session_id: String, url: String, provider: String) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    let label = format!("auth-{}", session_id.replace('-', ""));
    // 已存在则先关再开（重新发起）
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.close();
    }
    let title = format!("AI路由助手 · {} 授权登录", match provider.as_str() {
        "trae" => "Trae", "workbuddy" => "WorkBuddy", "minimax" => "MiniMax",
        "qoder" => "Qoder", "codearts" => "码道", "codearts-desktop" => "码道桌面", _ => "平台",
    });
    let win = WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(url.parse::<tauri::Url>().map_err(|e| e.to_string())?))
        .title(title)
        .inner_size(500.0, 680.0)
        .resizable(true)
        .center()
        // 允许所有导航；回调重定向到 127.0.0.1:port 时本机监听器直接接住完成授权
        .on_navigation(move |_| true)
        .build()
        .map_err(|e| e.to_string())?;
    // 拿到凭据后由前端调 login_close_window；这里也监听窗口被用户手动关闭的情况（无需处理，poll 会超时）
    let _ = win;
    Ok(())
}

#[tauri::command]
async fn login_close_window(app: tauri::AppHandle, session_id: String) -> Result<(), String> {
    let label = format!("auth-{}", session_id.replace('-', ""));
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.close();
    }
    Ok(())
}


// ===== 自动签到调度器（消费设置：auto/time/catchup） =====

async fn auto_tick(app: &tauri::AppHandle) -> Result<(), String> {
    let s = match svc() { Ok(s) => s, Err(e) => return Err(format!("svc: {e}")) };
    let get = |k: &str, d: &str| s.store.get_setting(k, d).unwrap_or_else(|_| d.to_string());
    let auto = get("auto", "1") == "1";
    if !auto { return Ok(()); }
    let time = get("time", "09:00");
    let random_time = get("randomTime", "0") == "1";
    // 当前北京时间
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64 + 8 * 3600;
    let day_secs = now % 86400;
    let (y, mo, d) = credit_core::adapters::civil_from_days(now / 86400);
    let today = format!("{y:04}-{mo:02}-{d:02}");
    let hhmm = format!("{:02}:{:02}", day_secs / 3600, (day_secs % 3600) / 60);
    // 随机模式：目标时间 = time + 0..59 分钟随机（每天固定种子简单处理：取 today hash % 60）
    let target = if random_time {
        let off = today.bytes().map(|b| b as u64).sum::<u64>() % 60;
        let h = time.get(0..2).and_then(|s| s.parse::<u64>().ok()).unwrap_or(9);
        let m = time.get(3..5).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        let total = h * 60 + m + off;
        format!("{:02}:{:02}", total / 60, total % 60)
    } else { time.clone() };
    let due = hhmm.as_str() >= target.as_str();
    let last_day = get("last_auto_run_day", "");
    let last_target = get("last_auto_run_target", "");
    // 今日已按当前目标跑过 → 跳过（目标时间变了重跑）
    if last_day == today && last_target == target { return Ok(()); }
    if !due { return Ok(()); }
    // 补跑判断（catchup）：错过时间（hhmm 已过 target 即 due=true，覆盖启动晚于计划时间的场景）
    let _ = get("catchup", "1");
    // 执行：auto=1 且今日未成功的账号
    s.store.set_setting("last_auto_run_day", &today);
    s.store.set_setting("last_auto_run_target", &target);
    let accounts = s.store.list_accounts().unwrap_or_default();
    let rows = s.store.list_accounts_full().unwrap_or_default();
    for (p, u, _, _, _) in accounts.iter() {
        let row = rows.iter().find(|r| r.provider == *p && r.uid == *u);
        let auto_on = row.map(|r| r.auto).unwrap_or(false);
        let today_done = row.map(|r| r.last_checkin.as_deref() == Some(today.as_str())).unwrap_or(false);
        if !auto_on || today_done { continue; }
        let outcome = s.checkin_account(p, u).await;
        if let Err(e) = &outcome {
            let _ = s.store.set_setting("last_auto_error", &format!("{p}:{u} failed {e}"));
        }
    }
    let _ = app;
    Ok(())
}

// ===== 模型网关（像本地 Ollama 一样对外提供 OpenAI 接口） =====

/// 网关端口（settings 表 gateway.port 可覆盖）
const GATEWAY_DEFAULT_PORT: u16 = 8756;

fn gw_router() -> Result<Router, String> {
    Ok(Router::new(Arc::new(Store::open_default().map_err(|e| e.to_string())?)))
}

#[tauri::command]
async fn gateway_status() -> Result<serde_json::Value, String> {
    let router = gw_router()?;
    let current = router.current_platform();
    let accounts: serde_json::Map<String, serde_json::Value> = Platform::all().iter()
        .map(|p| (p.as_str().to_string(), serde_json::json!(router.accounts_for(*p).len())))
        .collect();
    let s = svc()?;
    let port: u16 = s.store.get_setting("gatewayPort", &GATEWAY_DEFAULT_PORT.to_string())
        .ok().and_then(|v| v.parse().ok()).unwrap_or(GATEWAY_DEFAULT_PORT);
    let api_key = s.store.get_setting("gatewayApiKey", "").unwrap_or_default();
    Ok(serde_json::json!({
        "running": true,
        "port": port,
        "baseUrl": format!("http://127.0.0.1:{port}"),
        "platform": current.as_str(),
        "platformLabel": current.display(),
        "accounts": accounts,
        "rules": router.list_rules(),
        "auth": !api_key.is_empty(),
    }))
}

#[tauri::command]
async fn gateway_set_platform(platform: String) -> Result<serde_json::Value, String> {
    let router = gw_router()?;
    let p = Platform::parse(&platform).ok_or_else(|| format!("未知平台: {platform}"))?;
    router.set_current_platform(p);
    Ok(serde_json::json!({ "ok": true, "current": p.as_str(), "label": p.display() }))
}

#[tauri::command]
async fn gateway_add_rule(modelPattern: String, platform: String, account: String, weight: i64, enabled: bool) -> Result<serde_json::Value, String> {
    let router = gw_router()?;
    let rule = credit_gateway::RouteRule {
        id: String::new(),
        model_pattern: if modelPattern.is_empty() { "*".into() } else { modelPattern },
        platform,
        account,
        weight: weight.max(1) as u32,
        enabled,
        created_at: String::new(),
    };
    let saved = router.add_rule(rule);
    serde_json::to_value(saved).map_err(|e| e.to_string())
}

#[tauri::command]
async fn gateway_update_rule(rule: serde_json::Value) -> Result<(), String> {
    let router = gw_router()?;
    let r: credit_gateway::RouteRule = serde_json::from_value(rule).map_err(|e| e.to_string())?;
    if router.update_rule(r) { Ok(()) } else { Err("规则不存在".into()) }
}

#[tauri::command]
async fn gateway_delete_rule(id: String) -> Result<(), String> {
    let router = gw_router()?;
    if router.delete_rule(&id) { Ok(()) } else { Err("规则不存在".into()) }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    let h = handle.clone();
                    let _ = auto_tick(&h).await;
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                }
            });
            // 启动模型网关（axum，监听 127.0.0.1:8756；端口/密钥从 settings 读）
            tauri::async_runtime::spawn(async move {
                let store = match Store::open_default() {
                    Ok(s) => Arc::new(s),
                    Err(e) => { eprintln!("gateway store: {e}"); return; }
                };
                let port = store.get_setting("gatewayPort", &GATEWAY_DEFAULT_PORT.to_string())
                    .ok().and_then(|v| v.parse::<u16>().ok()).unwrap_or(GATEWAY_DEFAULT_PORT);
                let api_key = store.get_setting("gatewayApiKey", "").unwrap_or_default();
                let state = Arc::new(credit_gateway::GatewayState {
                    store: store.clone(),
                    router: Router::new(store),
                    api_key,
                });
                let addr = SocketAddr::from(([127, 0, 0, 1], port));
                if let Err(e) = credit_gateway::serve(state, addr).await {
                    eprintln!("gateway serve: {e}");
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            snapshot, import, checkin, quota_refresh, history,
            login_start, login_poll, login_cancel,
            account_delete, account_toggle_auto,
            statistics, daily_income, settings_get, settings_set, account_list_all, open_url,
            login_open_window, login_close_window,
            gateway_status, gateway_set_platform, gateway_add_rule, gateway_update_rule, gateway_delete_rule,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}