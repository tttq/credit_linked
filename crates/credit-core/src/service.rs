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

impl Service {
    /// 遇 Auth（401/过期）时尝试恢复凭据：minimax 优先读本机客户端最新 auth.json（本地客户端持续轮换 token），否则走 refresh 链。
    pub async fn refreshed_cred(&self, provider: &str, uid: &str, cred: &Cred) -> Result<Cred, PlatformError> {
        let adapter = adapter_for(provider).ok_or_else(|| PlatformError::Local(format!("no adapter for {provider}")))?;
        if provider == "minimax" {
            if let Some(live) = crate::adapters::minimax_local::read_live_auth() {
                let mut nc = crate::adapters::minimax_local::to_cred(live);
                nc.uid = uid.to_string();
                nc.name = cred.name.clone();
                if let Ok(auth) = serde_json::to_string(&nc) {
                    let _ = self.store.upsert_account(provider, uid, &nc.name, "", &auth);
                }
                return Ok(nc);
            }
        }
        let nc = adapter.refresh(cred).await?;
        if let Ok(auth) = serde_json::to_string(&nc) {
            let _ = self.store.upsert_account(provider, uid, &nc.name, "", &auth);
        }
        if let Some(exp) = &nc.expires_at {
            let _ = self.store.update_account_expires(provider, uid, Some(exp));
        }
        Ok(nc)
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
        let accounts = self.store.list_accounts().map_err(|e| PlatformError::Local(e.to_string()))?;
        let Some((_, _, name, secret_b64, auth)) = accounts.iter().find(|(p, u, _, _, _)| p == provider && u == uid) else {
            return Err(PlatformError::Local(format!("account {provider}:{uid} not found")));
        };
        let mut cred: Cred = serde_json::from_str(auth).map_err(|e| PlatformError::Local(format!("bad cred json: {e}")))?;
        let _ = secret_b64; // secret 字段保存加密凭据；本轮 cred 明文存 auth 列（单机 DPAPI 由 vault 处理）
        // trae 多账号设备号仲裁（去重/挑空闲）在请求前完成
        self.trae_pick_device(provider, uid, &mut cred);
        let adapter = adapter_for(provider).ok_or_else(|| PlatformError::Local(format!("no adapter for {provider}")))?;
        let cycle = crate::adapters::day_cn();

        // 当日已成功/已领 → 直接 already（幂等）；同时纠正状态列可能的残留失败态
        if let Ok(rows) = self.store.history_today(provider, uid, &cycle) {
            if !rows.is_empty() {
                let _ = self.store.update_account_status(provider, uid, "already", Some("今日已完成"));
                return Ok(CheckinOutcome { provider: provider.into(), uid: uid.into(), status: "already".into(), amount: None, message: Some("今日已完成".into()) });
            }
        }

        let outcome = match self.state_and_claim(provider, uid, &mut cred, adapter.as_ref()).await {
            Ok(o) => o,
            Err(e) => {
                // 平台链路失败也落库：Auth 类记 auth-expired（与 UI 徽标对应），其余记 failed
                let status = if matches!(e, PlatformError::Auth) { "auth-expired" } else { "failed" };
                CheckinOutcome { provider: provider.into(), uid: uid.into(), status: status.into(), amount: None, message: Some(e.to_string()) }
            }
        };
        let _ = self.store.record_history(provider, uid, name, &cycle, &outcome.status, outcome.amount, outcome.message.as_deref(), &crate::adapters::now_iso());
        let _ = self.store.update_account_status(provider, uid, &outcome.status, outcome.message.as_deref());
        Ok(outcome)
    }

    /// 签到公共链路：state →（Auth 时走刷新链恢复）→ claim →（设备冲突时换号重试）。
    /// 只有"账号不存在/凭据损坏"等本地错误向上抛；平台侧失败统一转成 outcome 由调用方落库。
    async fn state_and_claim(&self, provider: &str, uid: &str, cred: &mut Cred, adapter: &dyn Adapter) -> Result<CheckinOutcome, PlatformError> {
        let st = match adapter.state(cred).await {
            Ok(s) => s,
            Err(PlatformError::Auth) if cred.refresh_token.is_some() => {
                *cred = self.refreshed_cred(provider, uid, cred).await?;
                adapter.state(cred).await?
            }
            Err(e) => return Err(e),
        };
        let receipt = adapter.claim(cred, &st).await;
        // trae 设备冲突（9074/9095）自动换号重试；成功后记忆新设备号
        Ok(match receipt {
            Ok(r) => CheckinOutcome { provider: provider.into(), uid: uid.into(), status: r.status.clone(), amount: r.amount, message: r.message.clone() },
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("9074") || msg.contains("9095") {
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
    pub fn ui_snapshot(&self) -> Value {
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
                let new_uid = live.uid.clone();
                if let Ok(rows) = self.store.list_accounts() {
                    for (p, u, _, _, _) in rows {
                        if p == "zcode" && u != new_uid {
                            let _ = self.store.delete_account("zcode", &u);
                        }
                    }
                }
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
                // expiresAt 是 epoch 毫秒 → 转 ISO（显示用）
                let expires_iso = sess.pointer("/session/expiresAt").and_then(|e| e.as_i64()).map(|ms| {
                    let days = ms / 1000 / 86400;
                    let (y, mo, d) = crate::adapters::civil_from_days(days);
                    format!("{y:04}-{mo:02}-{d:02}")
                });
                let mut cred = crate::adapters::Cred {
                    provider: platform.into(),
                    uid: uid.clone(),
                    name: name.clone(),
                    token: String::new(),
                    // RT 入库：客户端回写的 RT 几乎总是最新可用的一条
                    refresh_token: sess.pointer("/session/refreshToken").and_then(|a| a.as_str()).map(|s| s.to_string()),
                    expires_at: expires_iso,
                    meta: json!({
                        "access_key_id": ak, "secret_access_key": sk, "security_token": st_,
                        // PKCE verifier + DPoP 私钥：refresh 续期必需
                        "verifier": sess.pointer("/loginContext/pkcePair/codeVerifier").and_then(|a| a.as_str()).unwrap_or_default(),
                        "privateJwk": sess.pointer("/loginContext/dpopKeyPair/privateKey").cloned().unwrap_or(Value::Null),
                        "client": if desktop { "desktop" } else { "agent" },
                    }),
                };
                if cred.uid.is_empty() {
                    cred.uid = format!("local-{}", if desktop { "desktop" } else { "agent" });
                }
                // verify 校验（caller-identity；失败不阻塞导入）
                let adapter = crate::adapters::codearts::CodeArtsAdapter { desktop };
                if let Ok(id) = crate::adapters::Adapter::verify(&adapter, &cred).await {
                    // caller-identity 返回 {account_id, principal_id, principal_urn}
                    if let Some(pn) = id.get("principal_urn").and_then(|u| u.as_str()) {
                        if let Some(n) = pn.rsplit(":user:").nth(1) {
                            if !n.is_empty() { cred.name = n.to_string(); }
                        }
                    }
                    if let Some(pid) = id.get("principal_id").and_then(|u| u.as_str()) {
                        if !pid.is_empty() && cred.uid.starts_with("local-") { cred.uid = pid.to_string(); }
                    }
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
