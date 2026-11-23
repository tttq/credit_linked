//! 服务编排：对齐原版 DesktopService.invoke 主路径（直连平台部分）。
//! 流程：读账号 → verify(TOCTOU) → state → 判重(cycle) → claim → 记 history → 更新状态。

use std::sync::Arc;

use serde_json::{json, Value};

use crate::adapters::*;
use crate::db::Store;
use crate::error::PlatformError;

/// 桌面服务：持有存储句柄，提供签到/额度/导入/统计等主编排入口。
pub struct Service {
    pub store: Arc<Store>,
}

/// 单账号签到结果：provider/uid 标识账号，status/amount/message 与 history 落库字段对齐。
pub struct CheckinOutcome {
    pub provider: String,
    pub uid: String,
    pub status: String, // success | already | failed | pending | needs-action | no-activity
    pub amount: Option<f64>,
    pub message: Option<String>,
}

/// 按平台标识构造对应 Adapter；未支持的平台返回 None。
pub fn adapter_for(provider: &str) -> Option<Box<dyn Adapter>> {
    match provider {
        "trae" => Some(Box::new(super::adapters::trae::TraeAdapter { req_source: 1 })),
        // trae-device：与 trae 相同，但签到走设备轮换路径（多账号各用各的已注册设备号）
        "trae-device" => Some(Box::new(super::adapters::trae::TraeAdapter { req_source: 1 })),
        "workbuddy" => Some(Box::new(super::adapters::workbuddy::WorkBuddyAdapter { intl: false })),
        "workbuddy-intl" => Some(Box::new(super::adapters::workbuddy::WorkBuddyAdapter { intl: true })),
        "minimax" => Some(Box::new(super::adapters::minimax::MiniMaxAdapter)),
        "qoder" => Some(Box::new(super::adapters::qoder::QoderAdapter { intl: false })),
        "qoder-intl" => Some(Box::new(super::adapters::qoder::QoderAdapter { intl: true })),
        "codearts" => Some(Box::new(super::adapters::codearts::CodeArtsAdapter { desktop: false })),
        "codearts-desktop" => Some(Box::new(super::adapters::codearts::CodeArtsAdapter { desktop: true })),
        "zcode" => Some(Box::new(super::adapters::zcode::ZcodeAdapter)),
        _ => None,
    }
}

// ============================ 签到串行化 / 连点去重 ============================

/// 同一账号签到互斥锁（进程内）：防止双击/并发调用重复领取
fn checkin_locks() -> &'static std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>> {
    static M: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>> = std::sync::OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn checkin_lock(key: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    let mut m = checkin_locks().lock().unwrap_or_else(|e| e.into_inner());
    m.entry(key.to_string()).or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(()))).clone()
}

// ==================== 凭据刷新单飞（single-flight） ====================
//
// 背景：MiniMax / CodeArts 这类平台的 refresh_token 是**一次性**的（用一次即轮换作废）。
// 若同一账号的签到与额度刷新并发触发 refresh，第二次必然拿到 `invalid_grant`
// —— 表现就是「一个账号签到成功，另一个账号报授权失效」。
// 做法：同一账号的 refresh 串行化；拿到锁后**重新读库**，若已被别人刷新过就直接复用。

fn cred_locks() -> &'static std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>> {
    static M: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>> = std::sync::OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn cred_lock(key: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    let mut m = cred_locks().lock().unwrap_or_else(|e| e.into_inner());
    m.entry(key.to_string()).or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(()))).clone()
}

/// 「刚刚才签到成功」的短时效标记（90s）：只用于连点幂等，
/// **不**参与跨窗口判重 —— 各平台领取窗口不同（Qoder 10:00 重置），按自然日判重会误伤。
const CHECKIN_RECENT_SECS: u64 = 90;

fn checkin_recent() -> &'static std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>> {
    static M: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>> = std::sync::OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn checkin_recent_success(key: &str) -> bool {
    let m = checkin_recent().lock().unwrap_or_else(|e| e.into_inner());
    m.get(key).map(|at| at.elapsed().as_secs() < CHECKIN_RECENT_SECS).unwrap_or(false)
}

fn checkin_mark_recent(key: &str) {
    let mut m = checkin_recent().lock().unwrap_or_else(|e| e.into_inner());
    m.insert(key.to_string(), std::time::Instant::now());
}

impl Service {
    /// 遇 Auth（401/过期）时尝试恢复凭据：minimax 优先读本机客户端最新 auth.json（本地客户端持续轮换 token），否则走 refresh 链。
    pub async fn refreshed_cred(&self, provider: &str, uid: &str, cred: &Cred) -> Result<Cred, PlatformError> {
        // 单飞：同一账号的刷新串行化（refresh_token 一次性，并发刷新必有一方 invalid_grant）
        let key = format!("{provider}:{uid}");
        let lock = cred_lock(&key);
        let _guard = lock.lock_owned().await;

        // 拿锁后重读库：若别的路径已经刷新过（token 变了），直接复用，不再消耗 RT
        if let Some(latest) = self.pick_cred(provider, uid) {
            if !latest.token.is_empty() && latest.token != cred.token {
                return Ok(latest);
            }
        }

        let adapter = adapter_for(provider).ok_or_else(|| PlatformError::Local(format!("no adapter for {provider}")))?;
        if provider == "minimax" {
            if let Some(live) = crate::adapters::minimax_local::read_live_auth() {
                let mut nc = crate::adapters::minimax_local::to_cred(live);
                nc.uid = uid.to_string();
                nc.name = cred.name.clone();
                // **只有本机 token 与当前不同（客户端真的轮换过）才采用**。
                // 旧实现无条件返回本机凭据：当本机文件里也是同一个过期 token 时，
                // 会把真正的 `adapter.refresh()` 短路掉 → minimax 永远停在 auth-expired。
                if !nc.token.is_empty() && nc.token != cred.token {
                    if let Ok(auth) = serde_json::to_string(&nc) {
                        let _ = self.store.upsert_account(provider, uid, &nc.name, "", &auth);
                    }
                    return Ok(nc);
                }
            }
        }
        let nc = match adapter.refresh(cred).await {
            Ok(nc) => nc,
            Err(e) => {
                // 一次性 RT 已作废：标记为需重新登录，避免后续巡检反复空转
                if matches!(&e, PlatformError::Business { code, .. } if code.as_str() == Some("invalid_grant")) {
                    let _ = self.store.update_account_status(provider, uid, "auth-expired",
                        Some("授权已失效（refresh_token 已轮换），请重新登录该平台客户端后再扫描导入"));
                }
                return Err(e);
            }
        };
        if let Ok(auth) = serde_json::to_string(&nc) {
            let _ = self.store.upsert_account(provider, uid, &nc.name, "", &auth);
        }
        if let Some(exp) = &nc.expires_at {
            let _ = self.store.update_account_expires(provider, uid, Some(exp));
        }
        Ok(nc)
    }

    /// 按 provider+uid 从库里取最新凭据
    fn pick_cred(&self, provider: &str, uid: &str) -> Option<Cred> {
        let rows = self.store.list_accounts().ok()?;
        rows.iter()
            .find(|(p, u, _, _, _)| p == provider && u == uid)
            .and_then(|(_, _, _, _, auth)| serde_json::from_str::<Cred>(auth).ok())
    }

    /// CodeArts 凭据自动续期（STS 一次性 RT 场景的标准做法）：
    /// ① 本机客户端会话已变化（客户端自己刷新过）→ 重新扫描导入；
    /// ② 客户端在运行且会话未变 → 等它刷新（最多 20s）后再导入；
    /// ③ 客户端没在跑 → 网关自己用 STS refresh（RT+PKCE+DPoP）续期，成功后
    ///    把新 AK/SK/ST/RT 落库并**回写客户端 secretStorage**（两边保持同一份 RT）；
    /// ④ 续期失败且客户端已安装 → 拉起客户端等它刷新/重新登录后再导入。
    pub async fn ensure_codearts_fresh(&self, cred: &Cred) -> Result<Cred, PlatformError> {
        self.ensure_codearts_fresh_opts(cred, true).await
    }

    /// `allow_wait=false`（后台巡检）：跳过「等客户端刷新」与「拉起客户端」两步，
    /// 只做「读本机会话 → STS refresh」。见 [`Self::heal_cred_quick`] 的说明。
    async fn ensure_codearts_fresh_opts(&self, cred: &Cred, allow_wait: bool) -> Result<Cred, PlatformError> {
        use crate::adapters::codearts_local as local;
        let desktop = cred.provider == "codearts-desktop";
        let stale_st = cred.meta.get("security_token").and_then(|v| v.as_str()).unwrap_or("").to_string();
        // ① 本地会话已更新 → 重新导入（使用磁盘上客户端回写的最新 STS）
        if let Some(live) = local::read_live_session(desktop).or_else(|| local::read_live_session(!desktop)) {
            let cur_st = live.session.pointer("/session/securityToken").and_then(|v| v.as_str()).unwrap_or("");
            if !cur_st.is_empty() && cur_st != stale_st
                && self.import_account(&cred.provider, false).await.is_ok() {
                    if let Some(c) = self.pick_cred(&cred.provider, &cred.uid) {
                        return Ok(c);
                    }
                }
            // ② 客户端在跑 → 等它按自己的周期刷新（不抢它的 RT）。后台巡检不等待（allow_wait=false）。
            if allow_wait && local::client_running() {
                let old_fp = local::session_fingerprint(desktop);
                let changed = tokio::task::spawn_blocking(move || {
                    local::wait_for_session_change(desktop, old_fp.as_deref(), std::time::Duration::from_secs(20))
                })
                .await
                .ok()
                .flatten();
                if changed.is_some() && self.import_account(&cred.provider, false).await.is_ok() {
                    if let Some(c) = self.pick_cred(&cred.provider, &cred.uid) {
                        return Ok(c);
                    }
                }
            }
        }
        // ③ 网关自己续期（客户端没在跑，或没等到客户端刷新）
        let adapter = adapter_for(&cred.provider)
            .ok_or_else(|| PlatformError::Local(format!("no adapter for {}", cred.provider)))?;
        match adapter.refresh(cred).await {
            Ok(nc) => {
                if let Ok(auth) = serde_json::to_string(&nc) {
                    let _ = self.store.upsert_account(&nc.provider, &nc.uid, &nc.name, "", &auth);
                }
                if let Some(exp) = &nc.expires_at {
                    let _ = self.store.update_account_expires(&nc.provider, &nc.uid, Some(exp));
                }
                // 回写客户端会话（best-effort；失败不阻塞请求）
                let ak = nc.meta.get("access_key_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let sk = nc.meta.get("secret_access_key").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let st = nc.meta.get("security_token").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let rt = nc.refresh_token.clone().unwrap_or_default();
                let exp_ms = nc.expires_at.as_deref().and_then(crate::adapters::codearts::iso_to_epoch_ms).unwrap_or(0);
                if !ak.is_empty() && !rt.is_empty() {
                    if let Err(e) = local::write_session_credentials(desktop, &ak, &sk, &st, &rt, exp_ms) {
                        eprintln!("[codearts] 续期成功但回写客户端会话失败: {e}");
                    }
                }
                Ok(nc)
            }
            Err(e) => {
                // ④ 兜底：拉起客户端等它刷新/重新登录（仅请求级；后台不拉起 GUI）
                let running = local::client_running();
                let has_exe = local::find_client_exe().is_some();
                if !running && has_exe && allow_wait {
                    eprintln!("[codearts] STS 续期失败（{e}），拉起客户端等待刷新…");
                    let old_fp = local::session_fingerprint(desktop);
                    local::launch_client();
                    let changed = tokio::task::spawn_blocking(move || {
                        local::wait_for_session_change(desktop, old_fp.as_deref(), std::time::Duration::from_secs(20))
                    })
                    .await
                    .ok()
                    .flatten();
                    if changed.is_some() && self.import_account(&cred.provider, false).await.is_ok() {
                        if let Some(c) = self.pick_cred(&cred.provider, &cred.uid) {
                            return Ok(c);
                        }
                    }
                }
                Err(e)
            }
        }
    }

    /// 全部平台通用的凭据自愈（**请求级**，用户正在等待 → 可以等客户端刷新/拉起客户端）。
    ///
    /// 统一链路（各平台一致，不再只对码道做）：
    /// ① **扫描本机**：客户端常会静默轮换 token → 直接重新导入最新登录态；
    /// ② **refresh 链**：用 refresh_token 主动换新；
    /// ③ **拉起客户端**：本地登录态也过期时拉起官方客户端，等它刷新后再扫一次。
    ///
    /// 返回最新可用凭据；全部失败返回 None。
    pub async fn heal_cred(&self, platform: &str, stale: &Cred) -> Option<Cred> {
        self.heal_cred_opts(platform, stale, true).await
    }

    /// 凭据自愈（**后台巡检用**）：只做「扫描本机 + refresh_token」，**不等客户端、不拉起 GUI**。
    ///
    /// 后台巡检必须是快速且无副作用的：
    /// - 等客户端刷新要 15~20s/账号，多个账号会把巡检拖到分钟级；
    /// - 拉起 GUI 客户端属于「打扰用户」的操作，只应在用户请求失败时做一次，
    ///   不该由后台定时器反复触发。
    /// 真正需要等待/拉起时，由请求命中 401 后的 [`Self::heal_cred`] 兜底。
    pub async fn heal_cred_quick(&self, platform: &str, stale: &Cred) -> Option<Cred> {
        self.heal_cred_opts(platform, stale, false).await
    }

    async fn heal_cred_opts(&self, platform: &str, stale: &Cred, allow_wait: bool) -> Option<Cred> {
        // 码道：STS 一次性 RT + 需回写客户端 secretStorage，链路特殊，复用既有实现
        if platform.starts_with("codearts") {
            return self.ensure_codearts_fresh_opts(stale, allow_wait).await.ok();
        }
        let (import_platform, intl) = import_target(platform);
        // ① 扫描本机（客户端可能已静默刷新）
        if self.import_account(import_platform, intl).await.is_ok() {
            if let Some(c) = self.pick_any_cred(platform, &stale.uid) {
                if !c.token.is_empty() {
                    return Some(c);
                }
            }
        }
        // ② refresh_token 换新
        if let Ok(nc) = self.refreshed_cred(platform, &stale.uid, stale).await {
            if !nc.token.is_empty() {
                return Some(nc);
            }
        }
        // ③ 拉起本机客户端，等它刷新登录态后重新扫描（仅请求级；后台不做）
        if !allow_wait {
            return None;
        }
        self.launch_and_rescan(platform).await
    }

    /// 取该 provider 的凭据：优先同 uid（常规刷新），其次任意（重新登录会换 uid）
    fn pick_any_cred(&self, provider: &str, uid: &str) -> Option<Cred> {
        if let Some(c) = self.pick_cred(provider, uid) {
            return Some(c);
        }
        let rows = self.store.list_accounts().ok()?;
        rows.iter()
            .filter(|(p, _, _, _, _)| p == provider)
            .find_map(|(_, _, _, _, auth)| serde_json::from_str::<Cred>(auth).ok())
    }

    /// 拉起本机平台客户端并等它刷新登录态（最长 15s），然后重新扫描导入。
    /// 进程拉起与轮询都在线程池里做，不阻塞异步运行时。
    async fn launch_and_rescan(&self, platform: &str) -> Option<Cred> {
        let p = platform.to_string();
        let old_fp = tokio::task::spawn_blocking({
            let p = p.clone();
            move || match p.as_str() {
                "trae" | "trae-device" | "trae-intl" => crate::adapters::trae_local::token_fingerprint(),
                "workbuddy" => crate::adapters::wb_local::session_fingerprint(false),
                "workbuddy-intl" => crate::adapters::wb_local::session_fingerprint(true),
                "qoder" => crate::adapters::qoder_local::token_fingerprint(false),
                "qoder-intl" => crate::adapters::qoder_local::token_fingerprint(true),
                "minimax" => crate::adapters::minimax_local::token_fingerprint(),
                "zcode" => crate::adapters::zcode_local::token_fingerprint(),
                _ => None,
            }
        })
        .await
        .ok()
        .flatten();

        let launched = tokio::task::spawn_blocking({
            let p = p.clone();
            move || match p.as_str() {
                "trae" | "trae-device" | "trae-intl" => crate::adapters::trae_local::launch_client().is_some(),
                "workbuddy" | "workbuddy-intl" => crate::adapters::wb_local::launch_client().is_some(),
                "qoder" | "qoder-intl" => crate::adapters::qoder_local::launch_client().is_some(),
                "minimax" => crate::adapters::minimax_local::launch_client().is_some(),
                "zcode" => crate::adapters::zcode_local::launch_client().is_some(),
                _ => false,
            }
        })
        .await
        .unwrap_or(false);
        if !launched {
            return None;
        }
        eprintln!("[heal] {platform} 凭据失效，已拉起本机客户端等待刷新…");
        let changed = tokio::task::spawn_blocking({
            let p = p.clone();
            let old = old_fp.clone();
            move || match p.as_str() {
                "trae" | "trae-device" | "trae-intl" => {
                    crate::adapters::trae_local::wait_for_token_change(old.as_deref(), std::time::Duration::from_secs(15)).is_some()
                }
                "workbuddy" | "workbuddy-intl" => {
                    crate::adapters::wb_local::wait_for_session_change(p == "workbuddy-intl", old.as_deref(), std::time::Duration::from_secs(15)).is_some()
                }
                "qoder" | "qoder-intl" => {
                    crate::adapters::qoder_local::wait_for_token_change(p == "qoder-intl", old.as_deref(), std::time::Duration::from_secs(15)).is_some()
                }
                "minimax" => crate::adapters::minimax_local::wait_for_token_change(old.as_deref(), std::time::Duration::from_secs(15)).is_some(),
                "zcode" => crate::adapters::zcode_local::wait_for_token_change(old.as_deref(), std::time::Duration::from_secs(15)).is_some(),
                _ => false,
            }
        })
        .await
        .unwrap_or(false);
        if !changed {
            return None;
        }
        let (import_platform, intl) = import_target(platform);
        if self.import_account(import_platform, intl).await.is_ok() {
            return self.pick_any_cred(platform, "");
        }
        None
    }

    /// 后台巡检用：把所有「本机已安装客户端」的登录态同步进库（不依赖过期判断）。
    ///
    /// 官方客户端会自己按周期轮换 token，但不会通知网关；定期重扫可保证库里
    /// 始终是当前有效凭据，从而**从源头避免**「用着用着突然 401/过期」。
    /// 返回 (provider, uid, 结果) 便于 UI 展示。
    pub async fn sync_from_local(&self) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        // 只同步库里已有账号的 provider（不擅自新增账号）
        let mut providers: Vec<String> = Vec::new();
        if let Ok(rows) = self.store.list_accounts() {
            for (p, _, _, _, _) in rows {
                if !providers.contains(&p) {
                    providers.push(p);
                }
            }
        }
        for p in providers {
            let (import_platform, intl) = import_target(&p);
            match self.import_account(import_platform, intl).await {
                Ok(_) => out.push((p.clone(), String::new(), "synced".into())),
                Err(e) => out.push((p.clone(), String::new(), format!("skip: {e}"))),
            }
        }
        out
    }

    /// 自动续期巡检（**全部平台**，不再只码道）：对「剩余有效期 < within 秒」的账号
    /// 执行 `heal_cred`。没有到期信息的平台跳过（由请求时的 401 自愈兜底）。
    /// `within` = 提前多久开始续期（秒）。
    ///
    /// 每个账号的续期都套**硬超时**：续期链会等官方客户端刷新（最长 15~20s）、
    /// 走 STS 换票，任一环节卡住都不能拖垮整个巡检（否则后台任务会永久停摆）。
    pub async fn renew_expiring(&self, within: i64) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        let Ok(rows) = self.store.list_accounts() else { return out };
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        for (p, u, _n, _s, auth) in rows {
            let Ok(cred) = serde_json::from_str::<Cred>(&auth) else { continue };
            let Some(exp_ms) = cred_expiry_ms(&p, &cred) else { continue };
            if exp_ms - now_ms > within * 1000 {
                continue;
            }
            let t0 = std::time::Instant::now();
            eprintln!("[renew] {p}:{u} 即将过期（剩余 {}s），开始自动续期…", (exp_ms - now_ms) / 1000);
            // 后台巡检走 **快速路径**：不等客户端刷新、不拉起 GUI 客户端（否则会拖到分钟级）。
            // 仍套硬超时兜底：万一底层阻塞调用卡住，也不会让巡检永久停摆。
            let healed = tokio::time::timeout(HEAL_TIMEOUT, self.heal_cred_quick(&p, &cred)).await;
            let dt = t0.elapsed().as_secs();
            match healed {
                Ok(Some(_)) => {
                    eprintln!("[renew] {p}:{u} 凭据已自动续期（{dt}s）");
                    out.push((p.clone(), u.clone(), "renewed".into()));
                }
                Ok(None) => {
                    eprintln!("[renew] {p}:{u} 凭据续期失败（{dt}s，需在客户端重新登录）");
                    out.push((p.clone(), u.clone(), "failed".into()));
                }
                Err(_) => {
                    eprintln!("[renew] {p}:{u} 凭据续期超时（>{dt}s），跳过以免阻塞后续巡检");
                    out.push((p.clone(), u.clone(), "timeout".into()));
                }
            }
        }
        out
    }
    /// trae 多账号设备号仲裁：上游 claim 校验 x-device-id 必须是已注册设备号，
    /// 且一个设备号一天只能签一个账号（9074/9095）。规则：
    /// ① 账号未配设备号 → 从本机已注册设备号里挑一个未被其他 trae 账号占用的；
    /// ② 全被占用 → 本轮不动（保持原样，后续手动设置）；
    /// ③ 两个账号撞了同一个号 → 后签的自动换成空闲号。
    fn trae_pick_device(&self, provider: &str, uid: &str, cred: &mut Cred) {
        if provider != "trae" { return; }
        let cur = cred.meta.get("deviceId").and_then(|d| d.as_str()).unwrap_or("").to_string();
        let others: Vec<String> = self.store.list_accounts().unwrap_or_default().into_iter()
            .filter(|(p, u, _, _, _)| p == "trae" && u != uid)
            .filter_map(|(_, _, _, _, auth)| serde_json::from_str::<Cred>(&auth).ok())
            .filter_map(|c| c.meta.get("deviceId").and_then(|d| d.as_str()).map(|s| s.to_string()))
            .filter(|s| !s.is_empty())
            .collect();
        let local = crate::adapters::trae_local::collect_local_device_ids();
        let taken = |id: &str| !id.is_empty() && others.iter().any(|o| o == id);
        // ① 未配置 → 挑空闲
        if cur.is_empty() {
            if let Some(free) = local.iter().find(|d| !taken(d)) {
                cred.meta["deviceId"] = json!(free);
            }
            return;
        }
        // ② 撞号 → 换空闲（无空闲则保持，交给手动设置/9095 提示）
        if taken(&cur) {
            if let Some(free) = local.iter().find(|d| !taken(d) && *d != &cur) {
                cred.meta["deviceId"] = json!(free);
            }
        }
    }

    /// 9074/9095 设备冲突自动恢复（trae）：按候选顺序换设备号重试 claim，成功即记忆该设备号。
    /// 候选顺序：本机空闲已注册设备号（最可能被服务端接受）→ 随机生成的新设备号。
    /// 只有签到成功才把新设备号写入账号凭据（下次直接使用）；失败不落库，避免污染原配置。
    async fn trae_rotate_device_retry(&self, provider: &str, uid: &str, name: &str, cred: &Cred, adapter: &dyn Adapter) -> Option<CheckinOutcome> {
        if provider != "trae" { return None; }
        let cur = cred.meta.get("deviceId").and_then(|d| d.as_str()).unwrap_or("").to_string();
        let others: Vec<String> = self.store.list_accounts().unwrap_or_default().into_iter()
            .filter(|(p, u, _, _, _)| p == "trae" && u != uid)
            .filter_map(|(_, _, _, _, auth)| serde_json::from_str::<Cred>(&auth).ok())
            .filter_map(|c| c.meta.get("deviceId").and_then(|d| d.as_str()).map(|s| s.to_string()))
            .filter(|s| !s.is_empty())
            .collect();
        let local = crate::adapters::trae_local::collect_local_device_ids();
        let mut candidates: Vec<String> = local.iter()
            .filter(|d| !d.is_empty() && **d != cur && !others.contains(d))
            .cloned()
            .collect();
        // 无空闲已注册号时，随机一个 16 位数字设备号兜底尝试
        candidates.push(crate::adapters::trae::new_device_identity().device_id);
        for dev in candidates {
            let mut nc = cred.clone();
            nc.meta["deviceId"] = json!(dev);
            let receipt = adapter.claim(&nc, &crate::adapters::CheckinState { claimable: true, ..Default::default() }).await;
            if let Ok(r) = receipt {
                // 签到成功：记住该设备号，下次签到直接使用
                if let Ok(auth) = serde_json::to_string(&nc) {
                    let _ = self.store.upsert_account(provider, uid, name, "", &auth);
                }
                let _ = self.store.record_history(provider, uid, name, &crate::adapters::day_cn(), &r.status, r.amount, r.message.as_deref(), &crate::adapters::now_iso());
                let _ = self.store.update_account_status(provider, uid, &r.status, r.message.as_deref());
                return Some(CheckinOutcome { provider: provider.into(), uid: uid.into(), status: r.status.clone(), amount: r.amount, message: r.message.clone() });
            }
        }
        Some(CheckinOutcome {
            provider: provider.into(), uid: uid.into(), status: "needs-action".into(), amount: None,
            message: Some("设备号冲突：已尝试本机空闲设备号与随机设备号均失败。Trae 一个设备号一天只能签一个账号，请重新登录该账号，或在账号管理里为各账号分别设置不同设备号".into()),
        })
    }

    /// 对单账号执行完整签到：当日已签直接幂等返回 → trae 设备号仲裁 → state/claim 链路 →
    /// 结果与状态落库（平台侧失败也转为 outcome 落库，不向上抛）。
    pub async fn checkin_account(&self, provider: &str, uid: &str) -> Result<CheckinOutcome, PlatformError> {
        // 同账号签到串行化 + 近期成功去重（双要点两次不会重复领取）
        let key = format!("{provider}:{uid}");
        let lock = checkin_lock(&key);
        // 近期已成功 → 直接幂等返回（仅防连点，不影响跨窗口的再次领取）
        if checkin_recent_success(&key) {
            return Ok(CheckinOutcome { provider: provider.into(), uid: uid.into(), status: "already".into(), amount: None, message: Some("刚刚已完成".into()) });
        }
        let _guard = lock.lock_owned().await;
        if checkin_recent_success(&key) {
            return Ok(CheckinOutcome { provider: provider.into(), uid: uid.into(), status: "already".into(), amount: None, message: Some("刚刚已完成".into()) });
        }

        let accounts = self.store.list_accounts().map_err(|e| PlatformError::Local(e.to_string()))?;
        let Some((_, _, name, secret_b64, auth)) = accounts.iter().find(|(p, u, _, _, _)| p == provider && u == uid) else {
            return Err(PlatformError::Local(format!("account {provider}:{uid} not found")));
        };
        let mut cred: Cred = serde_json::from_str(auth).map_err(|e| PlatformError::Local(format!("bad cred json: {e}")))?;
        let _ = secret_b64; // secret 字段保存加密凭据；本轮 cred 明文存 auth 列（单机 DPAPI 由 vault 处理）
        // trae 多账号设备号仲裁（去重/挑空闲）在请求前完成
        self.trae_pick_device(provider, uid, &mut cred);
        // 码道（CodeArts）：STS 有效期短（约 1h）且客户端会轮换 RT —— 签到前主动续期。
        // **但签到路径绝不等客户端、也不拉起客户端 GUI**（allow_wait=false）：
        // 旧实现用 allow_wait=true，会去 launch 客户端并长时间等待，导致「一键签到」整体挂死。
        // 这里再套 12s 硬超时，续期失败不阻塞 —— state/claim 链路里还有 Auth 恢复兜底。
        if provider.starts_with("codearts") {
            if let Ok(Ok(fresh)) = tokio::time::timeout(
                std::time::Duration::from_secs(12),
                self.ensure_codearts_fresh_opts(&cred, false),
            )
            .await
            {
                cred = fresh;
            }
        }
        let adapter = adapter_for(provider).ok_or_else(|| PlatformError::Local(format!("no adapter for {provider}")))?;
        let cycle = crate::adapters::day_cn();

        // **不再按「自然日」短路**：各平台的领取窗口并不都是零点重置
        // （如 Qoder「每天领 100 Credits」是 10:00(UTC+8) 开新窗口、次日 09:59 结束）。
        // 用自然日判重会在 00:00–09:59 之间把当天的真实领取误判成"今日已完成"，
        // 从而整天空转。这里改为**以服务端 state 为准**：服务端说可领就领，
        // 说已领就返回 already（各 adapter 的 claim 已内置 st.checked 判断）。
        let mut outcome = match self.state_and_claim(provider, uid, &mut cred, adapter.as_ref()).await {
            Ok(o) => o,
            Err(e) => {
                // 平台链路失败也落库：Auth 类记 auth-expired（与 UI 徽标对应），其余记 failed
                let status = if matches!(e, PlatformError::Auth) { "auth-expired" } else { "failed" };
                CheckinOutcome { provider: provider.into(), uid: uid.into(), status: status.into(), amount: None, message: Some(e.to_string()) }
            }
        };
        // 签到失败/授权过期 → 自动续期（重新扫描本机客户端会话 → STS RT 续期 → 拉起客户端）
        // 后整链重试一次；重试仍失败才落库失败态
        if matches!(outcome.status.as_str(), "failed" | "auth-expired") && provider.starts_with("codearts") {
            if let Ok(Ok(fresh)) = tokio::time::timeout(
                std::time::Duration::from_secs(12),
                self.ensure_codearts_fresh_opts(&cred, false),
            )
            .await
            {
                cred = fresh;
                match self.state_and_claim(provider, uid, &mut cred, adapter.as_ref()).await {
                    Ok(retry) if retry.status != "failed" && retry.status != "auth-expired" => {
                        outcome = retry;
                        outcome.message = outcome.message.take().map(|m| format!("自动续期后重签成功 · {m}"))
                            .or_else(|| Some("自动续期后重签成功".into()));
                    }
                    _ => {} // 重试仍失败：保留第一次失败结果落库
                }
            } // 续期失败：保留失败结果（message 已带平台原始错误）
        }
        let _ = self.store.record_history(provider, uid, name, &cycle, &outcome.status, outcome.amount, outcome.message.as_deref(), &crate::adapters::now_iso());
        let _ = self.store.update_account_status(provider, uid, &outcome.status, outcome.message.as_deref());
        // 连点幂等：成功（或服务端判定已领）都记一笔短时效标记
        if matches!(outcome.status.as_str(), "success" | "already") {
            checkin_mark_recent(&key);
        }
        Ok(outcome)
    }

    /// 签到公共链路：state →（Auth/1001 时走刷新链恢复）→ claim →（设备冲突换号 / 鉴权过期续期重试）。
    /// 只有"账号不存在/凭据损坏"等本地错误向上抛；平台侧失败统一转成 outcome 由调用方落库。
    ///
    /// Trae BUSINESS code=1001 = 鉴权失败（JWT 过期/无效，官方 1xxx 为鉴权错误段），
    /// 与设备号无关 —— 换 x-device-id 只对 9074/9095 有效。正确处理：refresh 换新 token
    /// 后重试一次；refresh 失败才落失败态（提示重新登录）。
    async fn state_and_claim(&self, provider: &str, uid: &str, cred: &mut Cred, adapter: &dyn Adapter) -> Result<CheckinOutcome, PlatformError> {
        // state 报 Auth（含 trae 1001 映射）→ 先续期再查状态。
        // **不再要求 refresh_token 存在**：minimax/trae/qoder/wb/codearts 的凭据主要靠
        // 「本机客户端轮换后重新扫描」恢复（refreshed_cred 内部先读本机会话，再走 refresh 链），
        // 旧条件 `refresh_token.is_some()` 会把「客户端还开着、只是 token 换了」的账号直接判死。
        let st = match adapter.state(cred).await {
            Ok(s) => s,
            Err(PlatformError::Auth) => {
                *cred = self.refreshed_cred(provider, uid, cred).await?;
                adapter.state(cred).await?
            }
            Err(e) => return Err(e),
        };
        let receipt = adapter.claim(cred, &st).await;
        // claim 报 BUSINESS code=1001/1002（token 过期）→ refresh 换票后重试一次（不换设备号）
        let receipt = match &receipt {
            Err(e) if is_auth_business_err(e) => {
                match self.refreshed_cred(provider, uid, cred).await {
                    Ok(nc) => {
                        *cred = nc;
                        adapter.claim(cred, &st).await
                    }
                    Err(_) => receipt, // 续期失败：保留原错误（提示重新登录）
                }
            }
            _ => receipt,
        };
        // trae 设备冲突（9074/9095）自动换号重试；成功后记忆新设备号
        Ok(match receipt {
            Ok(r) => CheckinOutcome { provider: provider.into(), uid: uid.into(), status: r.status.clone(), amount: r.amount, message: r.message.clone() },
            Err(e) => {
                let msg = e.to_string();
                if is_auth_business_err(&e) {
                    // 1001/1002 鉴权失败且无法续期：标记 auth-expired（与 UI「授权过期」徽标对应）
                    CheckinOutcome { provider: provider.into(), uid: uid.into(), status: "auth-expired".into(), amount: None, message: Some(format!("{msg}（token 已过期且无法自动续期，请重新登录 Trae 客户端后「扫描本机」）")) }
                } else if msg.contains("9074") || msg.contains("9095") {
                    if let Some(o) = self.trae_rotate_device_retry(provider, uid, &cred.name, cred, adapter).await {
                        o
                    } else {
                        CheckinOutcome { provider: provider.into(), uid: uid.into(), status: "failed".into(), amount: None, message: Some(msg) }
                    }
                } else {
                    CheckinOutcome { provider: provider.into(), uid: uid.into(), status: "failed".into(), amount: None, message: Some(msg) }
                }
            }
        })
    }

    /// 查询单账号额度：Auth 失效且有 refresh_token 时先恢复凭据再重试；结果持久化供启动展示。
    pub async fn quota_account(&self, provider: &str, uid: &str) -> Result<Quota, PlatformError> {
        let accounts = self.store.list_accounts().map_err(|e| PlatformError::Local(e.to_string()))?;
        let Some((_, _, _, _, auth)) = accounts.iter().find(|(p, u, _, _, _)| p == provider && u == uid) else {
            return Err(PlatformError::Local("account not found".into()));
        };
        let mut cred: Cred = serde_json::from_str(auth).map_err(|e| PlatformError::Local(e.to_string()))?;
        let adapter = adapter_for(provider).ok_or_else(|| PlatformError::Local("no adapter".into()))?;
        let q = match adapter.quota(&cred).await {
            Ok(q) => q,
            Err(PlatformError::Auth) if cred.refresh_token.is_some() => {
                cred = self.refreshed_cred(provider, uid, &cred).await?;
                adapter.quota(&cred).await?
            }
            Err(e) => return Err(e),
        };
        // 额度持久化：下次启动直接展示，无需手动点击
        if let Ok(qj) = serde_json::to_string(&q) {
            let _ = self.store.update_account_quota(provider, uid, &qj);
        }
        Ok(q)
    }

    /// 账号概要 JSON（provider/uid/name 列表）。
    pub fn state_summary(&self) -> Value {
        let accounts = self.store.list_accounts().unwrap_or_default();
        json!({
            "accounts": accounts.iter().map(|(p, u, n, _, _)| json!({ "provider": p, "uid": u, "name": n })).collect::<Vec<_>>(),
        })
    }

    /// 状态对账：以当日 history（success/already）为准纠正状态列残留。
    /// 兜底场景：此前设备冲突失败/历史版本未回写，导致"已签"账号仍显示签到失败。
    pub fn reconcile_statuses(&self) {
        let cycle = crate::adapters::day_cn();
        let Ok(accounts) = self.store.list_accounts_full() else { return };
        for a in accounts {
            if matches!(a.status.as_str(), "success" | "already") { continue; }
            if let Ok(rows) = self.store.history_today(&a.provider, &a.uid, &cycle) {
                if !rows.is_empty() {
                    let _ = self.store.update_account_status(&a.provider, &a.uid, "already", Some("今日已完成"));
                }
            }
        }
    }

    /// UI 快照：账号全行 + 最近历史。
    /// 清理脏账号行（uid 为空）：这类行是早期导入/同步写坏的数据，
    /// 会导致「一键签到」反复撞同一个不存在的账号、并把 bad cred 当成真实账号展示。
    pub fn prune_dirty_accounts(&self) -> usize {
        let Ok(rows) = self.store.list_accounts() else { return 0 };
        let mut n = 0;
        for (p, u, _n, _s, _a) in rows {
            if u.trim().is_empty() && self.store.delete_account(&p, &u).is_ok() {
                n += 1;
            }
        }
        n
    }

    pub fn ui_snapshot(&self) -> Value {
        self.prune_dirty_accounts();
        self.reconcile_statuses();
        let accounts = self.store.list_accounts_full().unwrap_or_default();
        let history = self.store.list_history(50).unwrap_or_default();
        json!({
            "accounts": accounts,
            "history": history,
            "cycle": crate::adapters::day_cn(),
        })
    }

    /// 本机扫描导入（trae/wb/qoder），对齐 CLI import 逻辑。
    pub async fn import_account(&self, platform: &str, intl: bool) -> Result<Value, PlatformError> {
        let cred = match platform {
            "trae" => {
                let Some(live) = crate::adapters::trae_local::read_live_auth() else {
                    return Err(PlatformError::Local("未发现可读取的 Trae 登录信息（未登录或未安装）".into()));
                };
                if live.is_intl {
                    return Err(PlatformError::Local(format!("检测到国际版登录态（host={}），当前仅支持国内版", live.host.unwrap_or_default())));
                }
                let token = live.auth.get("token").and_then(|t| t.as_str()).unwrap_or_default().to_string();
                if token.is_empty() {
                    return Err(PlatformError::Local("Trae 登录态 token 为空".into()));
                }
                let uid = jwt_uid_of(&token);
                // 身份头信息（对齐 dsh-connect-trae readTraeIdentity）：聊天请求必须带机器/设备/版本头
                let id = &live.identity;
                let mut meta = serde_json::json!({
                    "deviceId": live.device_id,
                    "host": live.host.unwrap_or_else(|| "https://api.trae.cn".into()),
                    "reqSource": crate::adapters::trae_local::request_source_for_dir(&live.data_dir),
                });
                if !id.machine_id.is_empty() { meta["machineId"] = json!(id.machine_id); }
                if !id.device_id.is_empty() { meta["deviceId"] = json!(id.device_id); }
                if let Some(v) = &id.app_version { meta["appVersion"] = json!(v); }
                if let Some(v) = &id.build_version { meta["buildVersion"] = json!(v); }
                if !id.os_version.is_empty() { meta["osVersion"] = json!(id.os_version); }
                crate::adapters::Cred {
                    provider: "trae".into(),
                    uid,
                    name: format!("Trae ({})", live.data_dir.file_name().unwrap_or_default().to_string_lossy()),
                    token,
                    refresh_token: live.auth.get("refreshToken").and_then(|t| t.as_str()).map(|s| s.to_string()),
                    expires_at: None,
                    meta,
                }
            }
            "wb" | "workbuddy" => {
                // 新版会话字段级加密：read_session_full 会在需要时拉起客户端取密钥解密
                let sess = crate::adapters::wb_local::read_session_full(intl)
                    .map_err(|e| PlatformError::Local(format!("读取 WorkBuddy 会话失败: {e}")))?
                    .ok_or_else(|| PlatformError::Local("未发现 WorkBuddy 会话文件（未登录或未安装）".into()))?;
                let acc = crate::adapters::wb_local::session_to_account(&sess)
                    .ok_or_else(|| PlatformError::Local("会话文件缺少 accessToken（客户端可能未登录）".into()))?;
                crate::adapters::Cred {
                    provider: if intl { "workbuddy-intl".into() } else { "workbuddy".into() },
                    uid: acc.get("uid").and_then(|u| u.as_str()).unwrap_or_default().to_string(),
                    name: acc.get("name").and_then(|n| n.as_str()).unwrap_or("WorkBuddy").to_string(),
                    token: acc.get("token").and_then(|t| t.as_str()).unwrap_or_default().to_string(),
                    refresh_token: acc.get("refreshToken").and_then(|t| t.as_str()).map(|s| s.to_string()),
                    expires_at: None,
                    meta: json!({
                        "domain": acc.get("domain").and_then(|d| d.as_str()).unwrap_or(if intl { "www.workbuddy.ai" } else { "www.codebuddy.cn" }),
                        "enterpriseId": acc.get("enterpriseId").and_then(|e| e.as_str()).unwrap_or_default(),
                    }),
                }
            }
            "qoder" => {
                let v = crate::adapters::qoder_local::read_auth(intl).map_err(|e| PlatformError::Local(e.to_string()))?
                    .ok_or_else(|| PlatformError::Local("未发现 Qoder auth.v1.dat（未登录或未安装）".into()))?;
                crate::adapters::Cred {
                    provider: if intl { "qoder-intl".into() } else { "qoder".into() },
                    uid: v.pointer("/user/id").and_then(|u| u.as_str()).map(|s| s.to_string())
                        .or_else(|| v.pointer("/user/id").map(|u| u.to_string().trim_matches('"').to_string()))
                        .unwrap_or_default(),
                    name: v.pointer("/user/name").and_then(|n| n.as_str()).unwrap_or("Qoder").to_string(),
                    token: v.get("token").and_then(|t| t.as_str()).unwrap_or_default().to_string(),
                    refresh_token: None,
                    expires_at: v.get("expiresAt").and_then(|e| e.as_str()).map(|s| s.to_string()),
                    meta: json!({}),
                }
            }
            "minimax" => {
                let live = crate::adapters::minimax_local::read_live_auth()
                    .ok_or_else(|| PlatformError::Local("未发现 MiniMax Code 登录信息（客户端未安装或未登录）".into()))?;
                crate::adapters::minimax_local::to_cred(live)
            }
            "zcode" => {
                let Some(live) = crate::adapters::zcode_local::read_live_auth() else {
                    return Err(PlatformError::Local("未发现 ZCode 登录信息（客户端未安装或未登录）".into()));
                };
                // ZCode 客户端同一时间只有一个登录账号：导入后清理旧 uid 账号（重登会换 uid）
                // 多账号：**不再删除**其它 ZCode 账号。
                // 每个账号各自保存自己的 zcodeJwt（meta.zcodeJwt），可分别签到/查额度；
                // 客户端同时只登录一个账号时，只有「本机当前登录」的那个能自动取票，
                // 其余账号保留记录并在状态里提示需重新登录，不再被静默删除。
                crate::adapters::Cred {
                    provider: "zcode".into(),
                    uid: live.uid,
                    name: live.name.unwrap_or_else(|| "ZCode (智谱)".into()),
                    token: live.access_token,
                    refresh_token: None,
                    // zcodejwttoken 是 JWT：解码 exp 填 expires_at（UI 显示用）
                    expires_at: live.zcode_jwt.as_deref().and_then(jwt_exp_of),
                    meta: json!({
                        "deviceMid": live.device_mid,
                        "zcodeJwt": live.zcode_jwt.unwrap_or_default(),
                    }),
                }
            }
            "codearts" | "codearts-desktop" => {
                let desktop = platform == "codearts-desktop";
                // 通道共用同一账号 STS 会话：找不到对应客户端时回退另一通道的本机会话
                let live = crate::adapters::codearts_local::read_live_session(desktop)
                    .or_else(|| crate::adapters::codearts_local::read_live_session(!desktop));
                let Some(live) = live else {
                    return Err(PlatformError::Local("未发现码道客户端登录信息（请先在码道客户端登录后再扫描，或改用「添加账号」走授权流程）".into()));
                };
                // 注意：read_live_session 返回的是 {loginContext, session} 根对象，字段在 session 下
                let sess = &live.session;
                let ak = sess.pointer("/session/accessKey").and_then(|a| a.as_str()).unwrap_or_default().to_string();
                let sk = sess.pointer("/session/secretKey").and_then(|a| a.as_str()).unwrap_or_default().to_string();
                let st_ = sess.pointer("/session/securityToken").and_then(|a| a.as_str()).unwrap_or_default().to_string();
                if ak.is_empty() || sk.is_empty() {
                    return Err(PlatformError::Local("码道会话缺少 accessKey/secretKey，请重新在客户端登录后再扫描".into()));
                }
                let uid = sess.pointer("/session/userId").and_then(|a| a.as_str()).unwrap_or_default().to_string();
                let name = sess.pointer("/session/userName").and_then(|a| a.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| if desktop { "码道桌面".into() } else { "华为云码道".into() });
                // expiresAt 是 epoch 毫秒 → 转完整 ISO（自动续期判断用）
                let expires_iso = sess.pointer("/session/expiresAt").and_then(|e| e.as_i64())
                    .map(crate::adapters::codearts::epoch_ms_to_iso);
                let rt = sess.pointer("/session/refreshToken").and_then(|a| a.as_str()).unwrap_or_default().to_string();
                let mut cred = crate::adapters::Cred {
                    provider: platform.into(),
                    uid: uid.clone(),
                    name: name.clone(),
                    token: String::new(),
                    // RT 入库：客户端回写的 RT 几乎总是最新可用的一条
                    refresh_token: if rt.is_empty() { None } else { Some(rt.clone()) },
                    expires_at: expires_iso,
                    meta: json!({
                        "access_key_id": ak, "secret_access_key": sk, "security_token": st_,
                        // PKCE verifier + DPoP 私钥：refresh 续期必需
                        "verifier": sess.pointer("/loginContext/pkcePair/codeVerifier").and_then(|a| a.as_str()).unwrap_or_default(),
                        "privateJwk": crate::adapters::codearts::jwk_value(
                            sess.pointer("/loginContext/dpopKeyPair/privateKey").unwrap_or(&Value::Null)
                        ),
                        "client": if desktop { "desktop" } else { "agent" },
                        "refresh_token": rt,
                    }),
                };
                if cred.uid.is_empty() {
                    cred.uid = format!("local-{}", if desktop { "desktop" } else { "agent" });
                }
                // verify 校验（caller-identity；失败不阻塞导入）：补齐规范化 uid/name/domain
                let adapter = crate::adapters::codearts::CodeArtsAdapter { desktop };
                if let Ok(id) = crate::adapters::Adapter::verify(&adapter, &cred).await {
                    let (pid, pname, domain) = crate::adapters::codearts::identity_of(&id);
                    if !pid.is_empty() && cred.uid.starts_with("local-") { cred.uid = pid; }
                    // 客户端会话里的 userName 是权威显示名；principal_urn 解析作为兜底
                    if (cred.name.is_empty() || cred.name == "华为云码道" || cred.name == "码道桌面")
                        && !pname.is_empty() { cred.name = pname; }
                    if !domain.is_empty() { cred.meta["domain_id"] = json!(domain); }
                }
                cred
            }
            other => return Err(PlatformError::Local(format!("platform {other} 暂不支持本机扫描导入（minimax/codearts 走登录流程，doubao 走窗口驱动）"))),
        };
        let auth = serde_json::to_string(&cred).map_err(|e| PlatformError::Local(e.to_string()))?;
        self.store.upsert_account(&cred.provider, &cred.uid, &cred.name, "", &auth)
            .map_err(|e| PlatformError::Local(e.to_string()))?;
        Ok(json!({ "provider": cred.provider, "uid": cred.uid, "name": cred.name }))
    }

/// 统计聚合：period = day|week|month|year，按 provider+账号过滤，返回分桶金额/次数。
pub fn statistics(&self, provider: Option<&str>, uid: Option<&str>, period: &str) -> Value {
    let rows = self.store.list_history(5000).unwrap_or_default();
    let mut buckets: std::collections::BTreeMap<String, (f64, i64, f64)> = Default::default(); // key → (sum, count, max)
    for h in rows {
        if let Some(p) = provider { if h.provider != p { continue; } }
        if let Some(u) = uid { if h.uid != u { continue; } }
        let day = h.at.get(..10).unwrap_or("").to_string();
        let key = match period {
            "day" => day.clone(),
            "week" => {
                let (y, w) = Self::iso_week(&day);
                format!("{y}-W{w:02}")
            }
            "month" => day.get(..7).unwrap_or("").to_string(),
            "year" => day.get(..4).unwrap_or("").to_string(),
            _ => day,
        };
        if key.is_empty() { continue; }
        let e = buckets.entry(key).or_insert((0.0, 0, 0.0));
        e.1 += 1;
        if h.status == "success" { e.0 += h.amount.unwrap_or(0.0); }
        if h.status == "success" { e.2 += h.amount.unwrap_or(0.0); }
    }
    let items: Vec<Value> = buckets.into_iter().map(|(k, (sum, cnt, _))| json!({
        "bucket": k, "sum": sum, "count": cnt,
    })).collect();
    json!({ "period": period, "items": items })
}

/// 按账号×日期的收入矩阵（首页图表用）：days 默认 30。
pub fn daily_income(&self, days: i64) -> Value {
    let rows = self.store.list_history(5000).unwrap_or_default();
    let mut per_day: std::collections::BTreeMap<String, f64> = Default::default();
    let mut per_prov_day: std::collections::BTreeMap<(String, String), f64> = Default::default();
    for h in rows {
        let day = h.at.get(..10).unwrap_or("").to_string();
        if day.is_empty() { continue; }
        if h.status == "success" {
            *per_day.entry(day.clone()).or_insert(0.0) += h.amount.unwrap_or(0.0);
            *per_prov_day.entry((day, h.provider.clone())).or_insert(0.0) += h.amount.unwrap_or(0.0);
        }
    }
    // 只保留最近 days 天
    let today = crate::adapters::day_cn();
    let mut items = Vec::new();
    for i in (0..days).rev() {
        let d = Self::shift_day(&today, -i);
        let mut by_prov = json!({});
        for ((day, p), v) in &per_prov_day {
            if *day == d { by_prov[p] = json!(v); }
        }
        items.push(json!({ "day": d, "income": per_day.get(&d).copied().unwrap_or(0.0), "by_provider": by_prov }));
    }
    json!({ "days": items })
}

fn iso_week(day: &str) -> (i64, u32) {
    // 简化 ISO 周：以 1970-01-01（周四）为基准的 7 天分桶，周一为界
    let parts: Vec<i64> = day.split('-').filter_map(|s| s.parse().ok()).collect();
    if parts.len() != 3 { return (0, 0); }
    let (y, m, d) = (parts[0], parts[1] as u32, parts[2] as u32);
    let days = crate::adapters::days_from_civil(y, m as i64, d as i64);
    // 1970-01-05 是周一
    let since = days - 4; // 1970-01-01 → days=0 → 1月5日=4
    let week = since.div_euclid(7);
    let mut wy = week % 52;
    if wy < 0 { wy += 52; }
    (1970 + week.div_euclid(52), (wy + 1) as u32)
}

fn shift_day(day: &str, delta: i64) -> String {
    let parts: Vec<i64> = day.split('-').filter_map(|s| s.parse().ok()).collect();
    if parts.len() != 3 { return day.to_string(); }
    let (y, m, d) = (parts[0], parts[1] as u32, parts[2] as u32);
    let days = crate::adapters::days_from_civil(y, m as i64, d as i64) + delta;
    let (y2, m2, d2) = crate::adapters::civil_from_days(days);
    format!("{y2:04}-{m2:02}-{d2:02}")
}
}

/// 鉴权类业务错误判定：Trae 1001（鉴权失败）/1002（令牌无效或过期）映射为 Auth 语义。
/// 上游签到/额度接口 token 过期时不再表现为普通 BUSINESS 失败，而是走 refresh 续期链。
fn is_auth_business_err(e: &PlatformError) -> bool {
    if matches!(e, PlatformError::Auth) {
        return true;
    }
    if let PlatformError::Business { code, .. } = e {
        return code.as_i64() == Some(1001) || code.as_i64() == Some(1002);
    }
    false
}

/// 单个账号凭据自愈的硬超时：自愈链会等客户端刷新（15~20s）+ 走 STS 换票，
/// 给 120s 足够；超时则跳过该账号，避免后台巡检被单个卡死账号拖住。
const HEAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// provider → (import_account 的平台名, 是否国际版)。/// `import_account` 按 (平台名, intl) 决定最终 provider，故国际版要拆开传。
fn import_target(provider: &str) -> (&'static str, bool) {
    match provider {
        "workbuddy-intl" => ("workbuddy", true),
        "workbuddy" => ("workbuddy", false),
        "qoder-intl" => ("qoder", true),
        "qoder" => ("qoder", false),
        "trae" | "trae-device" | "trae-intl" => ("trae", false),
        "minimax" => ("minimax", false),
        "zcode" => ("zcode", false),
        "codearts-desktop" => ("codearts-desktop", false),
        "codearts" => ("codearts", false),
        _ => ("", false),
    }
}

/// 凭据到期时间（epoch ms）。
///
/// 优先级：码道的**本机会话 expiresAt**（毫秒精度，最准）→ DB `expires_at`（ISO 8601）
/// → 仅日期（ZCode 的 zcodeJwt 只解出 YYYY-MM-DD，按当天 23:59:59+08 计）。
/// 都拿不到 → None（该平台交给请求时的 401 自愈兜底）。
fn cred_expiry_ms(provider: &str, cred: &Cred) -> Option<i64> {
    if provider.starts_with("codearts") {
        let desktop = provider == "codearts-desktop";
        if let Some(ms) = crate::adapters::codearts_local::read_live_session(desktop)
            .or_else(|| crate::adapters::codearts_local::read_live_session(!desktop))
            .and_then(|l| l.session.pointer("/session/expiresAt").and_then(|e| e.as_i64()))
        {
            return Some(ms);
        }
    }
    let s = cred.expires_at.as_deref()?.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(ms) = crate::adapters::codearts::iso_to_epoch_ms(s) {
        return Some(ms);
    }
    // 仅日期 YYYY-MM-DD → 该日结束（UTC）。ZCode 的 zcodeJwt exp 就是按 UTC 日期解出来的，
    // 故按 end-of-day UTC 作为上界即可（宁可略早续期，不要误判为已过期而反复拉起客户端）。
    let y: i64 = s.get(0..4)?.parse().ok()?;
    let mo: i64 = s.get(5..7)?.parse().ok()?;
    let d: i64 = s.get(8..10)?.parse().ok()?;
    let days = crate::adapters::days_from_civil(y, mo, d);
    Some((days + 1) * 86_400_000 - 1)
}

/// 从 Trae JWT payload 取 /data/id 作为 uid（解析失败返回空串）。
fn jwt_uid_of(token: &str) -> String {
    use base64::Engine;
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 { return String::new(); }
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[1]) else { return String::new() };
    let v: Value = serde_json::from_slice(&bytes).unwrap_or_default();
    v.pointer("/data/id").and_then(|i| i.as_str()).unwrap_or("").to_string()
}

/// JWT exp claim → ISO 日期（zcodejwttoken 用）
fn jwt_exp_of(token: &str) -> Option<String> {
    use base64::Engine;
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 { return None; }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[1]).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    let exp = v.get("exp").and_then(|e| e.as_i64())?;
    let (y, mo, d) = crate::adapters::civil_from_days(exp / 86400);
    Some(format!("{y:04}-{mo:02}-{d:02}"))
}
