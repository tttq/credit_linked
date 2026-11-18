#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Tauri 壳：rpc 命令面（对齐原版 credit:invoke 直连平台部分）。
//! snapshot / import / checkin / quota_refresh / history / login.start / login.poll / login.cancel / account.delete / account.toggle

use credit_core::db::Store;
use credit_core::service::Service;
use credit_gateway::protocol::Platform;
use credit_gateway::Router;
use std::net::SocketAddr;
use std::sync::Arc;
use tauri::{Manager, Emitter};

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

/// 全账号额度刷新（逐个拉取并持久化；单个失败不影响其余）
#[tauri::command]
async fn quota_refresh_all() -> Result<serde_json::Value, String> {
    let s = svc()?;
    let accounts = s.store.list_accounts().map_err(|e| e.to_string())?;
    let mut ok = 0usize;
    let mut fail = 0usize;
    for (p, uid, _, _, _) in &accounts {
        if p.starts_with("codearts") {
            // codearts 两个通道共用同一账号额度，只刷主通道避免重复
            if p == "codearts-desktop" { continue; }
        }
        match s.quota_account(p, uid).await {
            Ok(_) => ok += 1,
            Err(_) => fail += 1,
        }
    }
    Ok(serde_json::json!({ "ok": true, "refreshed": ok, "failed": fail }))
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
    let rpm: i64 = s.store.get_setting("gatewayRpm", "0").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let strict = s.store.get_setting("gatewayStrict", "1").ok().map(|v| v == "1").unwrap_or(true);
    let (total, ok, fail) = s.store.gateway_log_stats().unwrap_or((0, 0, 0));
    let logs: Vec<serde_json::Value> = s.store.list_gateway_logs(100).unwrap_or_default().into_iter()
        .map(|l| serde_json::json!({
            "id": l.id, "at": l.at, "platform": l.platform, "account": l.account,
            "model": l.model, "ok": l.ok, "status": l.status_code, "message": l.message, "tokens": l.tokens,
        })).collect();
    Ok(serde_json::json!({
        "running": true,
        "port": port,
        "baseUrl": format!("http://127.0.0.1:{port}"),
        "platform": current.as_str(),
        "platformLabel": current.display(),
        "accounts": accounts,
        "rules": router.list_rules(),
        "auth": !api_key.is_empty(),
        "rpm": rpm,
        "strict": strict,
        "logStats": { "total": total, "ok": ok, "fail": fail },
        "logs": logs,
    }))
}

/// 网关设置：端口 / 速率（RPM）/ 仅当前平台开关。端口改动需重启应用生效。
#[tauri::command]
async fn gateway_set_limits(port: Option<i64>, rpm: Option<i64>, strict: Option<bool>) -> Result<serde_json::Value, String> {
    let s = svc()?;
    let mut needs_restart = false;
    if let Some(p) = port {
        if !(1..=65535).contains(&p) {
            return Err("端口必须在 1-65535".into());
        }
        let old: i64 = s.store.get_setting("gatewayPort", &GATEWAY_DEFAULT_PORT.to_string())
            .ok().and_then(|v| v.parse().ok()).unwrap_or(GATEWAY_DEFAULT_PORT as i64);
        s.store.set_setting("gatewayPort", &p.to_string()).map_err(|e| e.to_string())?;
        needs_restart = p != old;
    }
    if let Some(r) = rpm {
        if r < 0 {
            return Err("RPM 不能为负数".into());
        }
        s.store.set_setting("gatewayRpm", &r.to_string()).map_err(|e| e.to_string())?;
    }
    if let Some(v) = strict {
        s.store.set_setting("gatewayStrict", if v { "1" } else { "0" }).map_err(|e| e.to_string())?;
    }
    Ok(serde_json::json!({ "ok": true, "needsRestart": needs_restart }))
}

/// 清空全部网关请求日志
#[tauri::command]
async fn gateway_clear_logs() -> Result<serde_json::Value, String> {
    let s = svc()?;
    let n = s.store.clear_gateway_logs().map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "ok": true, "cleared": n }))
}

// ===== 平台工具下载安装 =====

/// 检测平台客户端本机是否已安装（常见安装路径 + 客户端凭据探测）
fn tool_installed(platform: &str) -> bool {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let userprofile = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    // 下面所有路径用 '/'（Windows API 接受正斜杠），避免转义地狱
    // 卸载器/主程序常见位置（含用户自定义安装盘符的常见目录）
    let candidates: Vec<String> = match platform {
        "qoder" => vec![
            format!("{local}/Programs/Qoder/Qoder.exe"),
            format!("{local}/Programs/QoderWork/Qoder.exe"),
            format!("{local}/Programs/Qoder CN/Qoder CN.exe"),
            "D:/ProgramFiles/develop/Qoder CN/Qoder CN.exe".into(),
            "C:/Program Files/Qoder/Qoder.exe".into(),
        ],
        "minimax" => vec![
            format!("{local}/Programs/minimax-code/MiniMax Code.exe"),
            format!("{local}/Programs/MiniMax Code/MiniMax Code.exe"),
            "D:/ProgramFiles/develop/MiniMax Code/MiniMax Code.exe".into(),
            format!("{local}/minimax-code/MiniMax Code.exe"),
        ],
        "trae" => vec![
            format!("{local}/Programs/Trae/Trae.exe"),
            format!("{local}/Programs/Trae CN/Trae CN.exe"),
            "D:/ProgramFiles/develop/Trae CN/Trae CN.exe".into(),
            "C:/Program Files/Trae/Trae.exe".into(),
        ],
        "workbuddy" => vec![
            format!("{local}/Programs/CodeBuddy/CodeBuddy.exe"),
            "C:/Program Files/CodeBuddy/CodeBuddy.exe".into(),
        ],
        "zcode" => vec![
            format!("{local}/Programs/ZCode/ZCode.exe"),
            "D:/ProgramFiles/develop/ZCode/ZCode.exe".into(),
            "C:/Program Files/ZCode/ZCode.exe".into(),
        ],
        _ => vec![],
    };
    // 本机客户端凭据存在也算已安装（minimax ~/.minimax）
    if platform == "minimax" && std::path::Path::new(&userprofile).join(".minimax/auth/prod/cn/mcode-public/auth.json").exists() {
        return true;
    }
    // 卸载器存在也代表已安装（安装器都会装 Uninstall *.exe）
    let uninstallers: Vec<String> = match platform {
        "qoder" => vec!["D:/ProgramFiles/develop/Qoder CN/Uninstall Qoder CN.exe".into()],
        "minimax" => vec!["D:/ProgramFiles/develop/MiniMax Code/Uninstall MiniMax Code.exe".into()],
        _ => vec![],
    };
    candidates.iter().chain(uninstallers.iter()).any(|c| std::path::Path::new(c).exists())
}

/// 平台官方安装包直链（Windows x64）与下载页。url 为空时打开 page。
fn tool_urls(platform: &str) -> (Option<String>, &'static str, &'static str) {
    match platform {
        // Qoder：官方 latest 直链（NSIS 安装器，静默参数 /S）
        "qoder" => (Some("https://download.qoder.com/qoder-work/releases/latest/QoderWork-Setup-x64.exe".into()), "静默安装参数 /S", "https://qoder.com/download"),
        // MiniMax Code：首页 JS 里带版本号，latest 版本从 code.minimax.cn 抓；这里用官网页打开 + 抓最新版本接口由前端传入
        "minimax" => (None, "官网下载页", "https://code.minimax.cn/"),
        // Trae：JS 渲染无稳定直链 → 官方下载页
        "trae" => (None, "官网下载页", "https://www.trae.cn/downloads"),
        // WorkBuddy（腾讯 CodeBuddy）
        "workbuddy" => (None, "官网下载页", "https://www.codebuddy.cn/"),
        // ZCode（智谱）
        "zcode" => (None, "官网下载页", "https://zcode.bigmodel.cn/"),
        // CodeArts（华为）
        "codearts" | "codearts-desktop" => (None, "官网下载页", "https://developer.huawei.com/"),
        _ => (None, "", ""),
    }
}

/// 从 MiniMax 官网首页解析当前最新版安装包直链（版本号随发布变化）
async fn minimax_latest_url() -> Option<String> {
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).build().ok()?;
    let body = client.get("https://code.minimax.cn/").send().await.ok()?.text().await.ok()?;
    let re = regex_lite_minimax();
    re.find(&body).map(|m| m.as_str().to_string())
}

fn regex_lite_minimax() -> regex::Regex {
    regex::Regex::new(r#"https://file\.cdn\.minimax\.io/public/minimax-agent-prod/release/MiniMax%20Code%20Setup%20[^"'\s]+?x64\.exe"#).unwrap()
}

/// 下载进度快照（全局）
static DOWNLOADS: std::sync::Mutex<Vec<serde_json::Value>> = std::sync::Mutex::new(Vec::new());

#[tauri::command]
async fn tools_info() -> Result<serde_json::Value, String> {
    let s = svc()?;
    let install_dir = s.store.get_setting("toolsInstallDir", "").unwrap_or_default();
    // 各平台额度汇总（来自最近一次额度查询缓存，不现场打 API）
    let accounts = s.store.list_accounts_full().unwrap_or_default();
    let mut quota_sum: std::collections::HashMap<&str, (f64, i64, i64)> = std::collections::HashMap::new(); // total, accounts, with_quota
    for a in &accounts {
        let e = quota_sum.entry(a.provider.as_str()).or_insert((0.0, 0, 0));
        e.1 += 1;
        if let Ok(q) = serde_json::from_str::<serde_json::Value>(&a.quota.clone().unwrap_or_default()) {
            if let Some(r) = q.get("remaining").and_then(|x| x.as_f64()) {
                e.0 += r;
                e.2 += 1;
            }
        }
    }
    let mut out = Vec::new();
    for p in ["trae", "workbuddy", "minimax", "qoder", "zcode", "codearts"] {
        let (url, _mode, page) = tool_urls(p);
        let mut direct = url.is_some();
        let mut resolved = url.clone().unwrap_or_default();
        if p == "minimax" {
            if let Some(u) = minimax_latest_url().await { resolved = u; direct = true; }
        }
        let qs = quota_sum.get(p);
        let (codearts_total, codearts_count) = if p == "codearts" {
            let d = quota_sum.get("codearts-desktop");
            (qs.map(|x| x.0).unwrap_or(0.0) + d.map(|x| x.0).unwrap_or(0.0),
             qs.map(|x| x.1).unwrap_or(0) + d.map(|x| x.1).unwrap_or(0))
        } else { (0.0, 0) };
        let (total, count) = if p == "codearts" { (codearts_total, codearts_count) } else {
            qs.map(|x| (x.0, x.1)).unwrap_or((0.0, 0))
        };
        out.push(serde_json::json!({
            "platform": p,
            "direct": direct,
            "url": resolved,
            "page": page,
            "quotaTotal": if count > 0 { serde_json::json!(total) } else { serde_json::Value::Null },
            "accounts": count,
            "installed": tool_installed(p),
        }));
    }
    Ok(serde_json::json!({ "items": out, "downloads": DOWNLOADS.lock().unwrap().clone(), "installDir": install_dir }))
}

#[tauri::command]
async fn tools_download(app: tauri::AppHandle, platform: String, install_dir: String) -> Result<serde_json::Value, String> {
    let (url, mode, page) = tool_urls(&platform);
    let url = match url {
        Some(u) => u,
        None if platform == "minimax" => minimax_latest_url().await.ok_or("未获取到 MiniMax 最新版直链，请打开官网下载页")?,
        None => return Ok(serde_json::json!({ "ok": false, "openPage": page, "mode": mode })),
    };
    let file_name = url.split('/').last().unwrap_or("installer.exe").to_string();
    let dir = if install_dir.trim().is_empty() { std::env::temp_dir().join("credit-rs-installers").to_string_lossy().to_string() } else { install_dir };
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = std::path::Path::new(&dir).join(&file_name);
    let path_str = path.to_string_lossy().to_string();
    // 旧安装包先删（避免被运行中的安装器锁定导致写入失败）
    if path.exists() { let _ = std::fs::remove_file(&path); }

    let entry = serde_json::json!({ "platform": platform, "status": "downloading", "file": path_str, "progress": 0 });
    { let mut d = DOWNLOADS.lock().unwrap(); d.retain(|x| x["platform"] != platform); d.push(entry.clone()); }
    let _ = app.emit("tool-download", entry.clone());

    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(1800)).build().map_err(|e| e.to_string())?;
    let mut resp = client.get(&url).send().await.map_err(|e| e.to_string())?;
    let total = resp.content_length().unwrap_or(0);
    let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    use std::io::Write;
    let mut downloaded: u64 = 0;
    let mut last_emit = 0u64;
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        file.write_all(&chunk).map_err(|e| e.to_string())?;
        downloaded += chunk.len() as u64;
        let pct = if total > 0 { (downloaded * 100 / total) as u64 } else { 0 };
        if pct >= last_emit + 5 {
            last_emit = pct;
            let ev = serde_json::json!({ "platform": platform, "status": "downloading", "file": path_str, "progress": pct });
            { let mut d = DOWNLOADS.lock().unwrap(); if let Some(e) = d.iter_mut().find(|x| x["platform"] == platform) { *e = ev.clone(); } }
            let _ = app.emit("tool-download", ev);
        }
    }
    let done = serde_json::json!({ "platform": platform, "status": "downloaded", "file": path_str, "progress": 100 });
    { let mut d = DOWNLOADS.lock().unwrap(); if let Some(e) = d.iter_mut().find(|x| x["platform"] == platform) { *e = done.clone(); } }
    let _ = app.emit("tool-download", done.clone());
    Ok(serde_json::json!({ "ok": true, "file": path_str, "bytes": downloaded }))
}

/// 静默安装（NSIS: /S；Inno: /VERYSILENT /SUPPRESSMSGBOXES）—— 成功后提示用户重启工具
#[tauri::command]
async fn tools_install(platform: String, file: String) -> Result<serde_json::Value, String> {
    use std::process::Command;
    let lower = file.to_lowercase();
    let (args, kind) = if lower.contains("setup") || lower.ends_with(".exe") {
        (vec!["/S".to_string()], "nsis")
    } else {
        (vec!["/VERYSILENT".to_string(), "/SUPPRESSMSGBOXES".to_string()], "inno")
    };
    let status = Command::new(&file).args(&args).spawn().map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "ok": true, "kind": kind, "pid": status.id() }))
}

/// 打开平台官方下载页（无直链的平台引导）
#[tauri::command]
async fn tools_open_page(platform: String) -> Result<serde_json::Value, String> {
    let (_, _, page) = tool_urls(&platform);
    let _ = tauri_plugin_opener::open_url(page, None::<&str>);
    Ok(serde_json::json!({ "ok": true, "page": page }))
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
        .on_window_event(|window, event| {
            // 点关闭 → 隐藏到托盘常驻（不退出进程）
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                window.hide().ok();
                api.prevent_close();
            }
        })
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
                    rate_window: std::sync::Mutex::new(std::collections::VecDeque::new()),
                    port,
                });
                let addr = SocketAddr::from(([127, 0, 0, 1], port));
                if let Err(e) = credit_gateway::serve(state, addr).await {
                    eprintln!("gateway serve: {e}");
                }
            });

            // 托盘图标：左键单击显示主窗口；菜单 = 打开 / 退出
            use tauri::menu::{Menu, MenuItem};
            use tauri::tray::TrayIconBuilder;
            let show_item = MenuItem::with_id(app, "show", "打开 AI路由助手", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &quit_item])?;
            // 图标：优先窗口图标（windows 上为嵌入 ico），失败则显式解析 icon.ico
            let tray_icon = app.default_window_icon().cloned()
                .or_else(|| tauri::image::Image::from_bytes(include_bytes!("../icons/icon.ico")).ok())
                .or_else(|| tauri::image::Image::from_bytes(include_bytes!("../icons/icon.png")).ok());
            let mut builder = TrayIconBuilder::with_id("main-tray")
                .tooltip("AI路由助手 — 网关与自动签到运行中")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    match event.id.as_ref() {
                        "show" => {
                            if let Some(w) = app.get_webview_window("main") {
                                w.show().ok();
                                w.unminimize().ok();
                                w.set_focus().ok();
                            }
                        }
                        "quit" => { app.exit(0); }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click { button: tauri::tray::MouseButton::Left, button_state: tauri::tray::MouseButtonState::Up, .. } = event {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("main") {
                            w.show().ok();
                            w.unminimize().ok();
                            w.set_focus().ok();
                        }
                    }
                });
            if let Some(icon) = tray_icon {
                builder = builder.icon(icon);
            }
            builder.build(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            snapshot, import, checkin, quota_refresh, history,
            login_start, login_poll, login_cancel,
            account_delete, account_toggle_auto,
            statistics, daily_income, settings_get, settings_set, account_list_all, open_url,
            login_open_window, login_close_window,
            gateway_status, gateway_set_platform, gateway_add_rule, gateway_update_rule, gateway_delete_rule,
            gateway_set_limits, gateway_clear_logs, tools_info, tools_download, tools_install, tools_open_page, quota_refresh_all,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}