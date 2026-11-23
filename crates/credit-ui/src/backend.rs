//! backend — 命令逻辑层（原 WebView 壳的 #[command] 全部平移为普通 async fn）。
//! 每个业务动作对应一个 `pub fn` / `pub async fn`，UI 层经全局 tokio runtime 调用。
//!
//! 并发模型：gpui 的 `cx.spawn` 执行器不是 tokio runtime，直接 await 需要
//! tokio reactor 的函数（reqwest / tokio::spawn）会 panic。因此这里维护一个
//! 进程级全局 runtime，UI 层统一用 [`bg()`] 把 backend 异步函数派发到该
//! runtime 上执行，UI 侧只 await 返回的 JoinHandle（其 poll 不需要 runtime 上下文）。

use credit_core::db::Store;
use credit_core::service::Service;
use credit_gateway::protocol::Platform;
use credit_gateway::Router;
use std::net::SocketAddr;
use std::sync::Arc;

// ===== 全局 tokio runtime =====

static GLOBAL_RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();

fn global_rt() -> &'static tokio::runtime::Runtime {
    GLOBAL_RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .build()
            .expect("failed to build global tokio runtime")
    })
}

/// 在全局 tokio runtime 上派发一个异步任务，返回 JoinHandle 供 UI 侧 await。
/// 修复「no reactor running」panic：所有 backend 异步调用必须经此派发。
pub fn bg<F, R>(fut: F) -> tokio::task::JoinHandle<R>
where
    F: std::future::Future<Output = R> + Send + 'static,
    R: Send + 'static,
{
    global_rt().spawn(fut)
}

/// 打开默认数据库构造 Service（各命令共用入口）
pub fn svc() -> Result<Service, String> {
    Ok(Service { store: Arc::new(Store::open_default().map_err(|e| e.to_string())?) })
}

/// UI 快照：账号、签到状态与额度汇总（首页一次性数据）
pub async fn snapshot() -> Result<serde_json::Value, String> {
    let s = svc()?;
    Ok(s.ui_snapshot())
}

/// 扫描本机导入平台账号（intl = 国际版）
pub async fn import(platform: String, intl: bool) -> Result<serde_json::Value, String> {
    let s = svc()?;
    s.import_account(&platform, intl).await.map_err(|e| e.to_string())
}

/// 签到：单账号，或 all = 全部账号**串行**执行（一个完成后再处理下一个；单失败不影响其余）
///
/// 串行而非并发的原因：MiniMax / CodeArts 等平台的 refresh_token 是一次性的，
/// 并发执行会让同一账号的两次刷新互相作废（invalid_grant），
/// 也会撞上平台侧的「同设备串行」限制。串行换取稳定。
pub async fn checkin(provider: String, uid: String, all: bool) -> Result<serde_json::Value, String> {
    let s = svc()?;
    let ids: Vec<(String, String)> = if all {
        s.store.list_accounts().map_err(|e| e.to_string())?
            .into_iter().map(|(p, u, _, _, _)| (p, u)).collect()
    } else {
        vec![(provider, uid)]
    };
    let mut out = Vec::new();
    for (p, u) in ids {
        if u.trim().is_empty() { continue; }
        let name = p_name(&s, &p, &u);
        // 不设硬超时：签到一律等平台返回终态（网络慢/平台排队时可能较久）。
        // 串行执行本身已保证「一个账号完成后再处理下一个」，不需要超时兜底。
        match s.checkin_account(&p, &u).await {
            Ok(o) => out.push(serde_json::json!({
                "provider": o.provider, "uid": o.uid, "name": name,
                "status": o.status, "amount": o.amount, "message": o.message,
            })),
            Err(e) => out.push(serde_json::json!({
                "provider": p, "uid": u, "name": name, "status": "failed", "message": e.to_string(),
            })),
        }
    }
    Ok(serde_json::json!({ "results": out }))
}

/// 刷新单账号额度并持久化
pub async fn quota_refresh(provider: String, uid: String) -> Result<serde_json::Value, String> {
    let s = svc()?;
    let q = s.quota_account(&provider, &uid).await.map_err(|e| e.to_string())?;
    serde_json::to_value(q).map_err(|e| e.to_string())
}

/// 全账号额度刷新（逐个拉取并持久化；单个失败不影响其余）
pub async fn quota_refresh_all() -> Result<serde_json::Value, String> {
    let s = svc()?;
    let accounts = s.store.list_accounts().map_err(|e| e.to_string())?;
    let mut ok = 0usize;
    let mut fail = 0usize;
    for (p, uid, _, _, _) in &accounts {
        if p.starts_with("codearts") {
            if p == "codearts-desktop" { continue; } // 两通道共享额度，只刷主通道
        }
        match s.quota_account(p, uid).await {
            Ok(_) => ok += 1,
            Err(_) => fail += 1,
        }
    }
    Ok(serde_json::json!({ "ok": true, "refreshed": ok, "failed": fail }))
}

// ===== 自动更新 =====

const UPDATE_REPO: &str = "tttq/credit_linked";

/// 查询 GitHub Releases 最新版本，与当前版本比较
pub async fn check_update() -> Result<serde_json::Value, String> {
    let current = env!("CARGO_PKG_VERSION");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent("credit-rs-updater")
        .build().map_err(|e| e.to_string())?;
    let url = format!("https://api.github.com/repos/{}/releases/latest", UPDATE_REPO);
    let resp = client.get(&url).send().await.map_err(|e| format!("网络请求失败: {e}"))?;
    if resp.status().as_u16() == 404 {
        return Ok(serde_json::json!({ "updateAvailable": false, "reason": "no releases" }));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let tag = v.get("tag_name").and_then(|t| t.as_str()).unwrap_or("").trim_start_matches('v').to_string();
    let html_url = v.get("html_url").and_then(|u| u.as_str()).unwrap_or("").to_string();
    let notes = v.get("body").and_then(|b| b.as_str()).unwrap_or("").chars().take(500).collect::<String>();
    let parse = |s: &str| -> Vec<u64> { s.split('.').map(|x| x.parse().unwrap_or(0)).collect() };
    let cur = parse(current);
    let latest = parse(&tag);
    let update_available = {
        let mut less = false;
        for i in 0..cur.len().max(latest.len()) {
            let a = cur.get(i).copied().unwrap_or(0);
            let b = latest.get(i).copied().unwrap_or(0);
            if a != b { less = a < b; break; }
        }
        less
    };
    // 找 Windows x64 安装包 asset（nsis setup.exe）
    let mut asset_url = String::new();
    let mut asset_name = String::new();
    if let Some(assets) = v.get("assets").and_then(|a| a.as_array()) {
        for a in assets {
            let name = a.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let url = a.get("browser_download_url").and_then(|u| u.as_str()).unwrap_or("");
            if (name.contains("x64") || name.contains("x86_64")) && (name.ends_with(".exe") || name.ends_with(".msi")) && name.contains("setup") {
                asset_url = url.to_string();
                asset_name = name.to_string();
                break;
            }
        }
        if asset_url.is_empty() {
            for a in assets {
                let name = a.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let url = a.get("browser_download_url").and_then(|u| u.as_str()).unwrap_or("");
                if name.ends_with(".exe") && (name.contains("x64") || name.contains("x86_64")) {
                    asset_url = url.to_string();
                    asset_name = name.to_string();
                    break;
                }
            }
        }
    }
    Ok(serde_json::json!({
        "updateAvailable": update_available,
        "currentVersion": current,
        "latestVersion": tag,
        "releaseUrl": html_url,
        "notes": notes,
        "assetUrl": asset_url,
        "assetName": asset_name,
    }))
}

/// 下载更新进度回调（progress: 0-100, downloaded, total）
pub type ProgressCallback = Box<dyn Fn(u64, u64, u64) + Send + Sync>;

/// 下载更新安装包到临时目录（进度经回调抛给 UI）
pub async fn download_update(url: String, on_progress: Option<ProgressCallback>) -> Result<serde_json::Value, String> {
    let file_name = url.split('/').next_back().unwrap_or("update-setup.exe").to_string();
    let dir = std::env::temp_dir().join("credit-rs-update");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(&file_name);
    let path_str = path.to_string_lossy().to_string();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(1800))
        .user_agent("credit-rs-updater")
        .build().map_err(|e| e.to_string())?;
    let mut resp = client.get(&url).send().await.map_err(|e| e.to_string())?;
    let total = resp.content_length().unwrap_or(0);
    let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    use std::io::Write;
    let mut downloaded: u64 = 0;
    let mut last_emit = 0u64;
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        file.write_all(&chunk).map_err(|e| e.to_string())?;
        downloaded += chunk.len() as u64;
        let pct = downloaded.checked_mul(100).and_then(|n| n.checked_div(total)).unwrap_or(0);
        if pct >= last_emit + 5 {
            last_emit = pct;
            if let Some(cb) = &on_progress { cb(pct, downloaded, total); }
        }
    }
    Ok(serde_json::json!({ "ok": true, "file": path_str, "bytes": downloaded }))
}

/// 重启并安装：退出应用 + 启动安装包（NSIS /S 静默）
pub async fn install_update(file: String) -> Result<serde_json::Value, String> {
    if !std::path::Path::new(&file).exists() {
        return Err("安装包不存在".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let script = format!(
            "timeout /t 2 /nobreak >nul & start installer {} /S",
            file.replace('"', "")
        );
        let _ = std::process::Command::new("cmd")
            .args(["/C", &script])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .spawn();
    }
    #[cfg(not(windows))]
    let _ = &file;
    #[cfg(windows)]
    {
        if let Some(p) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())) {
            let _ = std::fs::remove_file(p.join("credit-tauri.update-pending"));
        }
    }
    std::process::exit(0);
    #[allow(unreachable_code)]
    Ok(serde_json::json!({ "ok": true }))
}

/// 退出应用（跨平台进程级退出）
pub async fn quit_app() -> Result<(), String> {
    std::process::exit(0);
}

/// 查询签到历史（默认最近 50 条）
pub async fn history(limit: Option<i64>) -> Result<serde_json::Value, String> {
    let s = svc()?;
    let rows = s.store.list_history(limit.unwrap_or(50)).map_err(|e| e.to_string())?;
    serde_json::to_value(rows).map_err(|e| e.to_string())
}

// ===== 登录 =====

/// 发起平台授权登录（返回 sessionId / 授权 URL / 有效期）
pub async fn login_start(provider: String) -> Result<serde_json::Value, String> {
    let (sid, url, exp) = credit_core::login::start_login(&provider).await.map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "sessionId": sid, "url": url, "expiresIn": exp }))
}

/// 轮询登录结果；拿到凭据时自动入库（同 provider+uid 覆盖）
pub async fn login_poll(session_id: String) -> Result<serde_json::Value, String> {
    let out = credit_core::login::poll_login(&session_id).await.map_err(|e| e.to_string())?;
    let mut v = serde_json::json!({ "status": out.status });
    if let Some(c) = &out.credential {
        let s = svc()?;
        let auth = serde_json::to_string(c).map_err(|e| e.to_string())?;
        s.store.upsert_account(&c.provider, &c.uid, &c.name, "", &auth).map_err(|e| e.to_string())?;
        if let Some(exp) = &c.expires_at {
            let _ = s.store.update_account_expires(&c.provider, &c.uid, Some(exp));
        }
        v["account"] = serde_json::json!({ "provider": c.provider, "uid": c.uid, "name": c.name });
    }
    if let Some(err) = &out.error {
        v["error"] = serde_json::json!(err);
    }
    Ok(v)
}

/// 取消指定会话的登录
pub async fn login_cancel_one(session_id: String) -> Result<(), String> {
    credit_core::login::cancel_one(&session_id);
    Ok(())
}

/// 取消全部进行中的登录会话
pub async fn login_cancel() -> Result<(), String> {
    credit_core::login::cancel_all();
    Ok(())
}

// ===== 账号管理 =====

pub async fn account_delete(provider: String, uid: String) -> Result<(), String> {
    let s = svc()?;
    s.store.delete_account(&provider, &uid).map_err(|e| e.to_string())
}

pub async fn account_toggle_auto(provider: String, uid: String, auto: bool) -> Result<(), String> {
    let s = svc()?;
    s.store.set_account_auto(&provider, &uid, auto).map_err(|e| e.to_string())
}

/// 从码道客户端 secretStorage 同步最新会话到已存账号
pub async fn codearts_sync_local() -> Result<serde_json::Value, String> {
    use credit_core::adapters::codearts_local;
    let mut synced = Vec::new();
    for desktop in [true, false] {
        let Some(live) = codearts_local::read_live_session(desktop) else { continue };
        let s = &live.session;
        let provider = if desktop { "codearts-desktop" } else { "codearts" };
        let uid = s.pointer("/session/userId").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        if uid.is_empty() { continue; }
        let s_svc = svc()?;
        let rows = s_svc.store.list_accounts().map_err(|e| e.to_string())?;
        if let Some((prov, uid_existing, name, _, auth)) = rows.iter().find(|(p, _, _, _, _)| p == provider) {
            let mut cred: credit_core::adapters::Cred = serde_json::from_str(auth).map_err(|e| e.to_string())?;
            cred.meta["access_key_id"] = s.pointer("/session/accessKey").cloned().unwrap_or(serde_json::Value::Null);
            cred.meta["secret_access_key"] = s.pointer("/session/secretKey").cloned().unwrap_or(serde_json::Value::Null);
            cred.meta["security_token"] = s.pointer("/session/securityToken").cloned().unwrap_or(serde_json::Value::Null);
            let rt = s.pointer("/session/refreshToken").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            cred.refresh_token = if rt.is_empty() { None } else { Some(rt.clone()) };
            cred.meta["refresh_token"] = serde_json::json!(rt);
            cred.meta["verifier"] = s.pointer("/loginContext/pkcePair/codeVerifier").cloned().unwrap_or(serde_json::Value::Null);
            cred.meta["privateJwk"] = credit_core::adapters::codearts::jwk_value(
                s.pointer("/loginContext/dpopKeyPair/privateKey").unwrap_or(&serde_json::Value::Null)
            );
            if let Some(uname) = s.pointer("/session/userName").and_then(|v| v.as_str()) {
                if !uname.is_empty() {
                    cred.name = uname.to_string();
                }
            }
            if let Some(ms) = s.pointer("/session/expiresAt").and_then(|e| e.as_i64()) {
                cred.expires_at = Some(credit_core::adapters::codearts::epoch_ms_to_iso(ms));
            }
            let auth_js = serde_json::to_string(&cred).map_err(|e| e.to_string())?;
            s_svc.store.upsert_account(prov, uid_existing, &cred.name, "", &auth_js).map_err(|e| e.to_string())?;
            let _ = name;
            synced.push(provider.to_string());
        }
    }
    Ok(serde_json::json!({ "ok": true, "synced": synced }))
}

pub async fn trae_local_devices() -> Result<Vec<String>, String> {
    Ok(credit_core::adapters::trae_local::collect_local_device_ids())
}

pub async fn trae_set_device(provider: String, uid: String, device_id: String) -> Result<(), String> {
    let s = svc()?;
    let accounts = s.store.list_accounts().map_err(|e| e.to_string())?;
    let Some((_, _, name, _, auth)) = accounts.iter().find(|(p, u, _, _, _)| p == &provider && u == &uid) else {
        return Err("账号不存在".into());
    };
    let mut cred: credit_core::adapters::Cred = serde_json::from_str(auth).map_err(|e| e.to_string())?;
    if device_id.is_empty() {
        cred.meta.as_object_mut().map(|o| o.remove("deviceId"));
    } else {
        if !device_id.chars().all(|c| c.is_ascii_digit()) || device_id.len() < 8 || device_id.len() > 20 {
            return Err("设备号应为 8~20 位纯数字（见本机 Trae 客户端 storage.json 的 iCubeAuthInfo://icube-dc: 键）".into());
        }
        cred.meta["deviceId"] = serde_json::json!(device_id);
    }
    s.store.upsert_account(&provider, &uid, name, "", &serde_json::to_string(&cred).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

/// 查账号显示名（不存在返回空串）
pub fn p_name(s: &Service, p: &str, u: &str) -> String {
    s.store.list_accounts().ok().and_then(|accs| {
        accs.into_iter().find(|(ap, au, _, _, _)| ap == p && au == u).map(|(_, _, n, _, _)| n)
    }).unwrap_or_default()
}

/// 签到/收益统计（可按平台、账号、周期 day/week/month 过滤）
pub async fn statistics(provider: Option<String>, uid: Option<String>, period: Option<String>) -> Result<serde_json::Value, String> {
    let s = svc()?;
    Ok(s.statistics(provider.as_deref(), uid.as_deref(), period.unwrap_or_else(|| "day".into()).as_str()))
}

/// 近 N 天每日收益（默认 7 天）
pub async fn daily_income(days: Option<i64>) -> Result<serde_json::Value, String> {
    let s = svc()?;
    Ok(s.daily_income(days.unwrap_or(7)))
}

/// 近 N 天每日 token 消耗
pub async fn daily_tokens(days: Option<i64>) -> Result<serde_json::Value, String> {
    let s = svc()?;
    let n = days.unwrap_or(7);
    let rows = s.store.token_stats_daily(n.max(1) * 2).unwrap_or_default();
    let mut map: std::collections::BTreeMap<String, (i64, i64, i64, i64)> = Default::default();
    for (day, p, c, t, req) in rows {
        map.insert(day, (p, c, t, req));
    }
    let today = credit_core::adapters::day_cn();
    let mut items = Vec::new();
    for i in (0..n).rev() {
        let day = shift_day_str(&today, -i);
        let (p, c, t, req) = map.remove(&day).unwrap_or((0, 0, 0, 0));
        items.push(serde_json::json!({
            "day": day, "promptTokens": p, "completionTokens": c, "totalTokens": t, "requests": req,
        }));
    }
    Ok(serde_json::json!({ "days": items }))
}

/// 日期平移（YYYY-MM-DD ± N 天）
pub fn shift_day_str(day: &str, delta: i64) -> String {
    let parts: Vec<i64> = day.split('-').filter_map(|x| x.parse().ok()).collect();
    if parts.len() != 3 { return day.to_string(); }
    let days = credit_core::adapters::days_from_civil(parts[0], parts[1], parts[2]) + delta;
    let (y, m, d) = credit_core::adapters::civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// 读取应用设置
pub async fn settings_get() -> Result<serde_json::Value, String> {
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
        "app": "AI Free Router",
        "schedTick": get("sched_tick", ""),
        "lastRunDay": get("last_auto_run_day", ""),
        "lastRunTarget": get("last_auto_run_target", ""),
        "lastAutoError": get("last_auto_error", ""),
        "locale": get("locale", ""),
        "theme": get("theme", ""),
    }))
}

/// 写单项设置
pub async fn settings_set(key: String, value: String) -> Result<(), String> {
    let s = svc()?;
    s.store.set_setting(&key, &value).map_err(|e| e.to_string())
}

/// 全量账号行（含状态/额度/最近签到时间）
pub async fn account_list_all() -> Result<serde_json::Value, String> {
    let s = svc()?;
    let rows = s.store.list_accounts_full().map_err(|e| e.to_string())?;
    serde_json::to_value(rows).map_err(|e| e.to_string())
}

/// 用系统默认浏览器打开 URL
pub async fn open_url(url: String) -> Result<(), String> {
    #[cfg(windows)]
    credit_core::proc_util::hide_console(std::process::Command::new("cmd"))
        .args(["/C", "start", "", &url]).spawn().map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(&url).spawn().map_err(|e| e.to_string())?;
    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open").arg(&url).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

// ===== 自动签到调度器 =====

/// 修复历史导入的码道账号元数据（名字 / DPoP 私钥 / RT / 到期时间）
fn repair_codearts_accounts(s: &Service) {
    use credit_core::adapters::codearts_local;
    let Ok(rows) = s.store.list_accounts() else { return };
    for (p, u, name, _, auth) in rows {
        if !p.starts_with("codearts") { continue; }
        let Ok(mut cred) = serde_json::from_str::<credit_core::adapters::Cred>(&auth) else { continue };
        let bad_name = name.starts_with("iam::") || name.is_empty();
        let bad_jwk = cred.meta.get("privateJwk").map(|j| j.is_string()).unwrap_or(true);
        let no_rt = cred.refresh_token.as_deref().unwrap_or("").is_empty();
        if !(bad_name || bad_jwk || no_rt) { continue; }
        let desktop = p == "codearts-desktop";
        let Some(live) = codearts_local::read_live_session(desktop)
            .or_else(|| codearts_local::read_live_session(!desktop))
        else { continue };
        if let Some(uname) = live.session.pointer("/session/userName").and_then(|v| v.as_str()) {
            if !uname.is_empty() { cred.name = uname.to_string(); }
        }
        let rt = live.session.pointer("/session/refreshToken").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        if !rt.is_empty() {
            cred.refresh_token = Some(rt.clone());
            cred.meta["refresh_token"] = serde_json::json!(rt);
        }
        cred.meta["privateJwk"] = credit_core::adapters::codearts::jwk_value(
            live.session.pointer("/loginContext/dpopKeyPair/privateKey").unwrap_or(&serde_json::Value::Null),
        );
        if let Some(ms) = live.session.pointer("/session/expiresAt").and_then(|e| e.as_i64()) {
            cred.expires_at = Some(credit_core::adapters::codearts::epoch_ms_to_iso(ms));
        }
        if let Ok(js) = serde_json::to_string(&cred) {
            let _ = s.store.upsert_account(&p, &u, &cred.name, "", &js);
        }
    }
}

/// 调度 tick：到计划时间（含随机偏移/补跑）后对开启自动且今日未签的账号执行签到
pub async fn auto_tick() -> Result<(), String> {
    let s = match svc() { Ok(s) => s, Err(e) => return Err(format!("svc: {e}")) };
    let get = |k: &str, d: &str| s.store.get_setting(k, d).unwrap_or_else(|_| d.to_string());
    repair_codearts_accounts(&s);
    // 凭据自动续期：每 10 分钟巡检（全平台，提前 30 分钟续期）
    let now_secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
    let last_renew: i64 = get("last_renew_tick", "0").parse().unwrap_or(0);
    if now_secs - last_renew >= 600 {
        let _ = s.store.set_setting("last_renew_tick", &now_secs.to_string());
        let _ = s.sync_from_local().await;
        for (p, u, r) in s.renew_expiring(1800).await {
            if r != "renewed" {
                let _ = s.store.set_setting("last_auto_error", &format!("{p}:{u} 凭据续期 {r}"));
            }
        }
    }
    let auto = get("auto", "1") == "1";
    if !auto { return Ok(()); }
    let time = get("time", "09:00");
    let random_time = get("randomTime", "0") == "1";
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64 + 8 * 3600;
    let day_secs = now % 86400;
    let (y, mo, d) = credit_core::adapters::civil_from_days(now / 86400);
    let today = format!("{y:04}-{mo:02}-{d:02}");
    let hhmm = format!("{:02}:{:02}", day_secs / 3600, (day_secs % 3600) / 60);
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
    if last_day == today && last_target == target { return Ok(()); }
    if !due { return Ok(()); }
    let _ = s.store.set_setting("last_auto_run_day", &today);
    let _ = s.store.set_setting("last_auto_run_target", &target);
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
    Ok(())
}

// ===== 模型网关 =====

const GATEWAY_DEFAULT_PORT: u16 = 8756;

pub fn gw_router() -> Result<Router, String> {
    Ok(Router::new(Arc::new(Store::open_default().map_err(|e| e.to_string())?)))
}

/// 网关状态 + 最近 100 条请求日志
pub async fn gateway_status() -> Result<serde_json::Value, String> {
    let router = gw_router()?;
    let current = router.current_platform();
    let mut accounts: serde_json::Map<String, serde_json::Value> = Platform::all().iter()
        .map(|p| (p.as_str().to_string(), serde_json::json!(router.accounts_for(*p).len())))
        .collect();
    let ca = router.accounts_for(Platform::CodeArts);
    let ca_agent = ca.iter().filter(|c| c.provider == "codearts").count();
    let ca_desktop = ca.iter().filter(|c| c.provider == "codearts-desktop").count();
    accounts.insert("codearts".into(), serde_json::json!(ca_agent));
    accounts.insert("codearts-desktop".into(), serde_json::json!(ca_desktop));
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
            "model": l.model, "ok": l.ok, "status": l.status_code, "message": l.message, "tokens": l.tokens, "duration": l.duration_ms,
        })).collect();
    Ok(serde_json::json!({
        "running": true,
        "port": port,
        "baseUrl": format!("http://127.0.0.1:{port}"),
        "platform": router.current_platform_raw(),
        "platformLabel": current.display(),
        "accounts": accounts,
        "rules": router.list_rules(),
        "auth": !api_key.is_empty(),
        "apiKey": api_key,
        "rpm": rpm,
        "strict": strict,
        "logStats": { "total": total, "ok": ok, "fail": fail },
        "logs": logs,
    }))
}

/// 网关设置：端口 / 速率（RPM）/ 仅当前平台开关
pub async fn gateway_set_limits(port: Option<i64>, rpm: Option<i64>, strict: Option<bool>) -> Result<serde_json::Value, String> {
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

pub async fn gateway_clear_logs() -> Result<serde_json::Value, String> {
    let s = svc()?;
    let n = s.store.clear_gateway_logs().map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "ok": true, "cleared": n }))
}

// ===== 平台工具下载安装 =====

/// 检测平台客户端本机是否已安装（注册表 Uninstall 键直查）
#[cfg(windows)]
fn registry_installed_entries() -> Vec<(String, String, String)> {
    use winreg::enums::*;
    use winreg::RegKey;
    let mut out: Vec<(String, String, String)> = Vec::new();
    let hives = [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER];
    let subkeys = [
        r"Software\Microsoft\Windows\CurrentVersion\Uninstall",
        r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
    ];
    for hive in hives {
        for sk in subkeys {
            let Ok(root) = RegKey::predef(hive).open_subkey_with_flags(sk, KEY_READ) else { continue };
            for key in root.enum_keys().flatten() {
                let Ok(k) = root.open_subkey_with_flags(&key, KEY_READ) else { continue };
                let name: String = k.get_value("DisplayName").unwrap_or_default();
                if name.is_empty() { continue; }
                let loc: String = k.get_value("InstallLocation").unwrap_or_default();
                let un: String = k.get_value("UninstallString").unwrap_or_default();
                out.push((name, loc, un));
            }
        }
    }
    out
}

#[cfg(not(windows))]
fn registry_installed_entries() -> Vec<(String, String, String)> {
    Vec::new()
}

fn installed_by_uninstaller(platform: &str, entries: &[(String, String, String)]) -> bool {
    entries.iter().any(|(name, _loc, _un)| {
        let n = name.to_lowercase();
        match platform {
            "qoder" => n.starts_with("qoder"),
            "minimax" => n.starts_with("minimax"),
            "trae" => n.starts_with("trae"),
            "workbuddy" => n.starts_with("workbuddy") || n.starts_with("codebuddy"),
            "zcode" => n.starts_with("zcode"),
            "codearts" | "codearts-desktop" => n.starts_with("codearts") || name.contains("码道"),
            _ => false,
        }
    })
}

fn tool_installed(platform: &str) -> bool {
    if let Ok(s) = svc() {
        if let Ok(dir) = s.store.get_setting(&format!("toolDir:{platform}"), "") {
            if !dir.is_empty() {
                let d = std::path::Path::new(&dir);
                if let Ok(rd) = std::fs::read_dir(d) {
                    for e in rd.flatten() {
                        let n: String = e.file_name().to_string_lossy().to_lowercase();
                        if n.ends_with(".exe") && (n.contains("uninstall") || main_exe_matches(platform, &n)) {
                            return true;
                        }
                    }
                }
            }
        }
    }
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let userprofile = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    let reg = registry_installed_entries();
    if installed_by_uninstaller(platform, &reg) { return true; }
    if let Some(hit) = reg.iter().find(|(name, _, _)| installed_by_uninstaller(platform, &[(name.clone(), String::new(), String::new())])) {
        let un = &hit.2;
        let exe_path = un.split('"').nth(1).unwrap_or(un.split(' ').next().unwrap_or(""));
        if !exe_path.is_empty() && std::path::Path::new(exe_path).exists() {
            return true;
        }
    }
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
        "codearts" | "codearts-desktop" => vec![
            "D:/ProgramFiles/software/CodeArts/CodeArts.exe".into(),
            "C:/Program Files/CodeArts/CodeArts.exe".into(),
            "C:/Program Files (x86)/CodeArts/CodeArts.exe".into(),
            format!("{local}/Programs/CodeArts/CodeArts.exe"),
        ],
        _ => vec![],
    };
    if platform == "minimax" && std::path::Path::new(&userprofile).join(".minimax/auth/prod/cn/mcode-public/auth.json").exists() {
        return true;
    }
    if platform.starts_with("codearts") {
        let appdata = std::env::var("APPDATA").unwrap_or_default();
        if std::path::Path::new(&appdata).join("codearts").join("Local State").exists() {
            return true;
        }
    }
    let uninstallers: Vec<String> = match platform {
        "qoder" => vec!["D:/ProgramFiles/develop/Qoder CN/Uninstall Qoder CN.exe".into()],
        "minimax" => vec!["D:/ProgramFiles/develop/MiniMax Code/Uninstall MiniMax Code.exe".into()],
        _ => vec![],
    };
    candidates.iter().chain(uninstallers.iter()).any(|c| std::path::Path::new(c).exists())
}

fn main_exe_matches(platform: &str, filename_lower: &str) -> bool {
    match platform {
        "qoder" => filename_lower.starts_with("qoder"),
        "minimax" => filename_lower.starts_with("minimax"),
        "trae" => filename_lower.starts_with("trae"),
        "workbuddy" => filename_lower.starts_with("workbuddy") || filename_lower.starts_with("codebuddy"),
        "zcode" => filename_lower.starts_with("zcode"),
        "codearts" | "codearts-desktop" => filename_lower.starts_with("codearts"),
        _ => false,
    }
}

/// 平台官方安装包直链（Windows x64）与下载页
fn tool_urls(platform: &str) -> (Option<String>, &'static str, &'static str) {
    match platform {
        "qoder" => (Some("https://download.qoder.com/qoder-work/releases/latest/QoderWork-Setup-x64.exe".into()), "静默安装参数 /S", "https://qoder.com/download"),
        "minimax" => (None, "官网下载页", "https://code.minimax.cn/"),
        "trae" => (None, "官网下载页", "https://www.trae.cn/downloads"),
        "workbuddy" => (None, "官网下载页", "https://www.codebuddy.cn/"),
        "zcode" => (None, "官网下载页", "https://zcode.bigmodel.cn/"),
        "codearts" | "codearts-desktop" => (None, "官网下载页", "https://developer.huawei.com/"),
        _ => (None, "", ""),
    }
}

static MINIMAX_URL_CACHE: std::sync::OnceLock<std::sync::Mutex<Option<String>>> = std::sync::OnceLock::new();

/// MiniMax Code 最新版直链：优先 GitHub Releases API（按版本号排序取最新 x64 exe），
/// 失败时回落官网页面正则探测。
async fn minimax_latest_url() -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent("credit-rs-updater")
        .build().ok()?;

    // 1) GitHub Releases 最新版的 Windows x64 安装包
    if let Ok(resp) = client
        .get("https://api.github.com/repos/MiniMax-AI/MiniMax-Code/releases/latest")
        .send().await
    {
        if let Ok(v) = resp.json::<serde_json::Value>().await {
            if let Some(assets) = v.get("assets").and_then(|a| a.as_array()) {
                for a in assets {
                    let name = a.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let url = a.get("browser_download_url").and_then(|u| u.as_str()).unwrap_or("");
                    let lower = name.to_lowercase();
                    if lower.ends_with(".exe")
                        && (lower.contains("x64") || lower.contains("win") || lower.contains("setup"))
                    {
                        return Some(url.to_string());
                    }
                }
            }
        }
    }

    // 2) 回落：官网页面探测（保留原逻辑）
    let body = client.get("https://code.minimax.cn/").send().await.ok()?.text().await.ok()?;
    regex_lite_minimax().find(&body).map(|m| m.as_str().to_string())
}

fn regex_lite_minimax() -> regex::Regex {
    regex::Regex::new(r#"https://file\.cdn\.minimax\.io/public/minimax-agent-prod/release/MiniMax%20Code%20Setup%20[^"'\s]+?x64\.exe"#).unwrap()
}

/// 下载进度快照（全局）
static DOWNLOADS: std::sync::Mutex<Vec<serde_json::Value>> = std::sync::Mutex::new(Vec::new());

/// 工具下载进度回调（platform, status, file, progress）
pub type ToolDownloadCallback = Arc<dyn Fn(String, String, String, i64) + Send + Sync>;

/// 安装包默认下载目录：优先用户配置，否则用系统临时目录下的 credit-rs-installers
fn installer_dir_setting(s: &Service) -> String {
    let configured = s.store.get_setting("toolsInstallDir", "").unwrap_or_default();
    if !configured.trim().is_empty() {
        return configured;
    }
    let dir = std::env::temp_dir().join("credit-rs-installers");
    let _ = std::fs::create_dir_all(&dir);
    dir.to_string_lossy().to_string()
}

pub async fn tools_info() -> Result<serde_json::Value, String> {
    let s = svc()?;
    let install_dir = installer_dir_setting(&s);
    let accounts = s.store.list_accounts_full().unwrap_or_default();
    let mut quota_sum: std::collections::HashMap<&str, (f64, i64, i64)> = std::collections::HashMap::new();
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
            let cache = MINIMAX_URL_CACHE.get_or_init(|| std::sync::Mutex::new(None));
            if let Ok(guard) = cache.lock() {
                if let Some(u) = guard.clone() {
                    resolved = u; direct = true;
                }
            }
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
    let downloads = DOWNLOADS.lock().map(|d| d.clone()).unwrap_or_default();
    Ok(serde_json::json!({ "items": out, "downloads": downloads, "installDir": install_dir }))
}

/// 手动导入平台安装目录
pub async fn tools_set_install_dir(platform: String, dir: String) -> Result<serde_json::Value, String> {
    let platform: &str = &platform;
    let s = svc()?;
    let key = format!("toolDir:{platform}");
    if dir.trim().is_empty() {
        let _ = s.store.set_setting(&key, "");
        return Ok(serde_json::json!({ "ok": true, "installed": false, "cleared": true }));
    }
    let d = std::path::Path::new(dir.trim());
    if !d.is_dir() {
        return Err("目录不存在，请选择客户端安装目录（含主 exe）".into());
    }
    let mut found = String::new();
    if let Ok(rd) = std::fs::read_dir(d) {
        for e in rd.flatten() {
            let n: String = e.file_name().to_string_lossy().to_lowercase();
            if n.ends_with(".exe") && (n.contains("uninstall") || main_exe_matches(platform, &n)) {
                found = e.file_name().to_string_lossy().to_string();
                break;
            }
        }
    }
    if found.is_empty() {
        return Err(format!("目录里没有找到 {} 的主程序/卸载器 exe", platform));
    }
    s.store.set_setting(&key, dir.trim()).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "ok": true, "installed": true, "exe": found }))
}

/// 后台探测 MiniMax 最新版直链并缓存
pub async fn tools_refresh_minimax_url() -> Result<(), String> {
    if let Some(u) = minimax_latest_url().await {
        let cache = MINIMAX_URL_CACHE.get_or_init(|| std::sync::Mutex::new(None));
        if let Ok(mut g) = cache.lock() { *g = Some(u); }
    }
    Ok(())
}

/// 下载平台安装包（进度经回调抛给 UI）
pub async fn tools_download(platform: String, install_dir: String, on_progress: Option<ToolDownloadCallback>) -> Result<serde_json::Value, String> {
    let (url, mode, page) = tool_urls(&platform);
    let url = match url {
        Some(u) => u,
        None if platform == "minimax" => minimax_latest_url().await.ok_or("未获取到 MiniMax 最新版直链，请打开官网下载页")?,
        None => return Ok(serde_json::json!({ "ok": false, "openPage": page, "mode": mode })),
    };
    let file_name = url.split('/').next_back().unwrap_or("installer.exe").to_string();
    let dir = if install_dir.trim().is_empty() { std::env::temp_dir().join("credit-rs-installers").to_string_lossy().to_string() } else { install_dir };
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = std::path::Path::new(&dir).join(&file_name);
    let path_str = path.to_string_lossy().to_string();
    if path.exists() { let _ = std::fs::remove_file(&path); }

    let entry = serde_json::json!({ "platform": platform, "status": "downloading", "file": path_str, "progress": 0 });
    { let mut d = DOWNLOADS.lock().unwrap(); d.retain(|x| x["platform"] != platform); d.push(entry.clone()); }
    if let Some(cb) = &on_progress { cb(platform.clone(), "downloading".into(), path_str.clone(), 0); }

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
        let pct = downloaded.checked_mul(100).and_then(|n| n.checked_div(total)).unwrap_or(0);
        if pct >= last_emit + 5 {
            last_emit = pct;
            let ev = serde_json::json!({ "platform": platform, "status": "downloading", "file": path_str, "progress": pct });
            { let mut d = DOWNLOADS.lock().unwrap(); if let Some(e) = d.iter_mut().find(|x| x["platform"] == platform) { *e = ev.clone(); } }
            if let Some(cb) = &on_progress { cb(platform.clone(), "downloading".into(), path_str.clone(), pct as i64); }
        }
    }
    let done = serde_json::json!({ "platform": platform, "status": "downloaded", "file": path_str, "progress": 100 });
    { let mut d = DOWNLOADS.lock().unwrap(); if let Some(e) = d.iter_mut().find(|x| x["platform"] == platform) { *e = done.clone(); } }
    if let Some(cb) = &on_progress { cb(platform.clone(), "downloaded".into(), path_str.clone(), 100); }
    Ok(serde_json::json!({ "ok": true, "file": path_str, "bytes": downloaded }))
}

/// 静默安装（NSIS: /S）
pub async fn tools_install(_platform: String, file: String) -> Result<serde_json::Value, String> {
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

pub async fn tools_open_page(platform: String) -> Result<serde_json::Value, String> {
    let (_, _, page) = tool_urls(&platform);
    open_url(page.to_string()).await?;
    Ok(serde_json::json!({ "ok": true, "page": page }))
}

/// 平台客户端进程名与候选启动路径
fn client_process_info(platform: &str) -> (&'static str, Vec<String>) {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    match platform {
        "qoder" => ("Qoder CN", vec![
            "D:\\ProgramFiles\\develop\\Qoder CN\\Qoder CN.exe".into(),
            format!("{local}\\Programs\\Qoder\\Qoder.exe"),
        ]),
        "minimax" => ("MiniMax Code", vec![
            "D:\\ProgramFiles\\develop\\MiniMax Code\\MiniMax Code.exe".into(),
            format!("{local}\\Programs\\MiniMax Code\\MiniMax Code.exe"),
        ]),
        "trae" => ("Trae CN", vec![
            "D:\\ProgramFiles\\develop\\Trae CN\\Trae CN.exe".into(),
            format!("{local}\\Programs\\Trae\\Trae.exe"),
        ]),
        "workbuddy" => ("WorkBuddy", vec![
            "D:\\ProgramFiles\\software\\WorkBuddy\\WorkBuddy.exe".into(),
        ]),
        "zcode" => ("ZCode", vec![
            "D:\\ProgramFiles\\develop\\ZCode\\ZCode.exe".into(),
        ]),
        "codearts" | "codearts-desktop" => ("CodeArts", vec![
            "D:\\ProgramFiles\\software\\CodeArts\\CodeArts.exe".into(),
        ]),
        _ => ("", vec![]),
    }
}

/// 检查平台客户端是否运行；未运行则启动它
pub async fn platform_ensure_running(platform: String) -> Result<serde_json::Value, String> {
    use std::process::Command;
    let (proc_name, exe_paths) = client_process_info(&platform);
    if proc_name.is_empty() {
        return Ok(serde_json::json!({ "ok": false, "reason": "unknown platform" }));
    }
    let mut tl = Command::new("tasklist");
    tl.args(["/FI", &format!("IMAGENAME eq {}", exe_paths.first().map(|p| p.rsplit('\\').next().unwrap_or(proc_name)).unwrap_or(proc_name)), "/FO", "CSV", "/NH"]);
    let running = credit_core::proc_util::hide_console(tl)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(proc_name))
        .unwrap_or(false);
    if running {
        return Ok(serde_json::json!({ "ok": true, "running": true, "launched": false }));
    }
    let exe = exe_paths.iter().find(|p| std::path::Path::new(p).exists());
    match exe {
        Some(p) => {
            Command::new(p).spawn().map_err(|e| format!("启动失败: {e}"))?;
            Ok(serde_json::json!({ "ok": true, "running": false, "launched": true, "exe": p }))
        }
        None => Ok(serde_json::json!({ "ok": false, "reason": "client exe not found", "running": false, "launched": false })),
    }
}

pub async fn gateway_set_platform(platform: String) -> Result<serde_json::Value, String> {
    if credit_gateway::protocol::Platform::parse(&platform).is_none() {
        return Err(format!("未知平台: {platform}"));
    }
    let router = gw_router()?;
    router.set_current_platform_str(&platform);
    let p = credit_gateway::protocol::Platform::parse(&platform).unwrap();
    Ok(serde_json::json!({ "ok": true, "current": platform, "label": p.display() }))
}

pub async fn gateway_accounts() -> Result<serde_json::Value, String> {
    let router = gw_router()?;
    let raw = router.current_platform_raw();
    let provider = if raw == "codearts-desktop" { "codearts-desktop" } else { Platform::parse(&raw).unwrap().account_provider() };
    let s = svc()?;
    let full = s.store.list_accounts_full().unwrap_or_default();
    let rows = s.store.list_accounts().map_err(|e| e.to_string())?;
    let chosen = s.store.get_setting("gw_account", "").unwrap_or_default();
    let accounts: Vec<serde_json::Value> = rows.into_iter()
        .filter(|(prov, _, _, _, _)| prov == provider)
        .map(|(prov, uid, name, _, _)| {
            let row = full.iter().find(|r| r.provider == prov && r.uid == uid);
            let quota: Option<serde_json::Value> = row.and_then(|r| {
                r.quota.as_ref().and_then(|q| serde_json::from_str(q).ok())
            });
            let remaining = quota.as_ref().and_then(|q| q.get("remaining").and_then(|v| v.as_f64()));
            serde_json::json!({
                "ref": format!("{prov}:{uid}"), "provider": prov, "uid": uid,
                "name": name, "status": row.map(|r| r.status.as_str()).unwrap_or(""),
                "remaining": remaining,
                "active": chosen == format!("{prov}:{uid}"),
            })
        }).collect();
    Ok(serde_json::json!({
        "platform": raw, "provider": provider,
        "active": chosen, "accounts": accounts,
    }))
}

pub async fn gateway_switch_account(account_ref: Option<String>) -> Result<serde_json::Value, String> {
    let router = gw_router()?;
    let raw = router.current_platform_raw();
    let provider = if raw == "codearts-desktop" { "codearts-desktop".to_string() } else { Platform::parse(&raw).unwrap().account_provider().to_string() };
    let s = svc()?;
    let rows = s.store.list_accounts().map_err(|e| e.to_string())?;
    let valid: Vec<String> = rows.iter()
        .filter(|(prov, _, _, _, _)| prov == &provider)
        .map(|(prov, uid, _, _, _)| format!("{prov}:{uid}"))
        .collect();
    let chosen = account_ref.unwrap_or_default();
    if !chosen.is_empty() && !valid.contains(&chosen) {
        return Err(format!("账号 {chosen} 不属于平台 {provider}"));
    }
    s.store.set_setting("gw_account", &chosen).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "ok": true, "platform": raw, "active": chosen, "available": valid.len() }))
}

pub async fn gateway_add_rule(model_pattern: String, platform: String, account: String, weight: i64, enabled: bool) -> Result<serde_json::Value, String> {
    let router = gw_router()?;
    let rule = credit_gateway::RouteRule {
        id: String::new(),
        model_pattern: if model_pattern.is_empty() { "*".into() } else { model_pattern },
        platform,
        account,
        weight: weight.max(1) as u32,
        enabled,
        created_at: String::new(),
    };
    let saved = router.add_rule(rule);
    serde_json::to_value(saved).map_err(|e| e.to_string())
}

pub async fn gateway_update_rule(rule: serde_json::Value) -> Result<(), String> {
    let router = gw_router()?;
    let r: credit_gateway::RouteRule = serde_json::from_value(rule).map_err(|e| e.to_string())?;
    if router.update_rule(r) { Ok(()) } else { Err("规则不存在".into()) }
}

pub async fn gateway_delete_rule(id: String) -> Result<(), String> {
    let router = gw_router()?;
    if router.delete_rule(&id) { Ok(()) } else { Err("规则不存在".into()) }
}

/// 启动模型网关服务（axum，独立线程 runtime）
pub fn spawn_gateway() {
    std::thread::Builder::new().name("gateway".into()).spawn(|| {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => { eprintln!("gateway runtime: {e}"); return; }
        };
        rt.block_on(async {
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
                device_salt: std::sync::Mutex::new(credit_gateway::server::device_salt_init()),
                device_token: std::sync::Mutex::new(None),
                models_cache: std::sync::Mutex::new(None),
                models_info: std::sync::Mutex::new(std::collections::HashMap::new()),
            });
            let addr = SocketAddr::from(([127, 0, 0, 1], port));
            if let Err(e) = credit_gateway::serve(state, addr).await {
                eprintln!("gateway serve: {e}");
            }
        });
    }).expect("spawn gateway thread");
}

/// 启动自动签到调度循环（60s tick）
pub fn spawn_auto_tick() {
    std::thread::Builder::new().name("auto-tick".into()).spawn(|| {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => { eprintln!("auto-tick runtime: {e}"); return; }
        };
        rt.block_on(async {
            loop {
                let _ = auto_tick().await;
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }
        });
    }).expect("spawn auto-tick thread");
}
