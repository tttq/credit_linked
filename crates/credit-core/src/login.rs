//! 平台登录编排（对齐原版 adapters/login.ts + vendor traeAuth.mjs 协议）。
//! 会话仅存内存：start_login 打开授权页 → poll_login 轮询（trae/codearts 由本机回调任务自动完成）→ 完成。
//! 豆包/国际线按原版提示走「导入」。

use crate::adapters::Cred;
use crate::error::PlatformError;
use crate::net::http::platform_fetch;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

// ===== 会话数据 =====

/// 登录会话数据：按平台区分进行中状态；Done 表示后台回调任务已写回结果。
#[derive(Clone)]
pub enum FlowData {
    /// trae: 回调监听已在后台任务中跑，完成/失败后写回 shared。
    Trae { shared: Arc<Mutex<Option<Result<Cred, String>>>> },
    /// minimax: device flow（poll 里直接打 token 端点）
    Mini { verifier: String, device_code: String, interval_secs: u64 },
    /// qoder: openapi deviceToken/poll
    Qoder { verifier: String, nonce: String },
    /// workbuddy: codebuddy auth state 轮询
    Wb { state: String },
    /// codearts: 回调监听后台任务 + 完成写回 shared
    CodeArts { shared: Arc<Mutex<Option<Result<Cred, String>>>> },
    /// 已完成（trae/codearts 后台任务写入）
    Done(Result<Cred, String>),
}

/// 一次登录会话：session_id 供轮询、url 供打开授权页，5 分钟过期。
#[derive(Clone)]
pub struct Flow {
    pub session_id: String,
    pub provider: String,
    pub url: String,
    pub expires_at: u64,
    pub data: FlowData,
}

static FLOWS: std::sync::OnceLock<Mutex<HashMap<String, Flow>>> = std::sync::OnceLock::new();

fn flows() -> &'static Mutex<HashMap<String, Flow>> {
    FLOWS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

/// 清空全部登录会话（应用退出或重置时调用）。
pub fn cancel_all() {
    if let Ok(mut m) = flows().lock() {
        m.clear();
    }
}

fn new_id() -> String { uuid::Uuid::new_v4().to_string() }

fn rand_b64url(n: usize) -> String {
    let mut b = vec![0u8; n];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut b);
    B64URL.encode(b)
}

fn hex_string(n: usize) -> String {
    let mut b = vec![0u8; n];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut b);
    b.iter().map(|x| format!("{x:02x}")).collect::<String>()
}

// ===== 极小回调服务器 =====

async fn wait_callback(listener: &TcpListener, timeout: std::time::Duration, path_hint: &str, reply: &str) -> Result<String, PlatformError> {
    let mut buf = vec![0u8; 8192];
    loop {
        let (mut s, _) = tokio::time::timeout(timeout, listener.accept()).await
            .map_err(|_| PlatformError::Local("登录回调等待超时（5 分钟未完成）".into()))?
            .map_err(|e| PlatformError::Local(format!("回调接受失败: {e}")))?;
        let n = s.read(&mut buf).await.map_err(|e| PlatformError::Local(e.to_string()))?;
        let req = String::from_utf8_lossy(&buf[..n]).to_string();
        let first = req.lines().next().unwrap_or("");
        let target = first.split_whitespace().nth(1).unwrap_or("/");
        let (path, query) = match target.split_once('?') {
            Some((p, q)) => (p.to_string(), q.to_string()),
            None => (target.to_string(), String::new()),
        };
        if path != path_hint {
            let body = "此登录请求已失效";
            let _ = s.write_all(format!("HTTP/1.1 404 Not Found\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).await;
            continue; // 探测/其他路径：继续等真正的回调
        }
        // 官方：secret 中转请求需 307 跳转到 redirect（校验域名白名单），只消费一次
        let qmap = parse_query(&query);
        if qmap.contains_key("secret") && !qmap.contains_key("code") {
            let redirect = qmap.get("redirect").cloned().unwrap_or_default();
            let allowed = redirect.starts_with("https://auth.huaweicloud.com/")
                || redirect.starts_with("https://auth.cn-north-4.myhuaweicloud.com/")
                || redirect.starts_with("https://console.huaweicloud.com/");
            if allowed {
                let body = "redirecting";
                let _ = s.write_all(format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: {redirect}\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).await;
            } else {
                let body = "redirect host not allowed";
                let _ = s.write_all(format!("HTTP/1.1 403 Forbidden\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).await;
            }
            continue;
        }
        let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: *\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{reply}", reply.len()).as_bytes()).await;
        return Ok(query);
    }
}

/// 解析回调 URL 查询串为键值表（复用 trae 的解析实现）。
pub fn parse_query(raw: &str) -> HashMap<String, String> {
    crate::adapters::trae::parse_callback_query(raw)
}

// ===== 启动登录 =====

/// 返回 (sessionId, url, expiresIn 秒)
pub async fn start_login(provider: &str) -> Result<(String, String, u64), PlatformError> {
    match provider {
        "doubao" | "doubao-work" => Err(PlatformError::Local("豆包网页登录由正式桌面端独立窗口处理".into())),
        "workbuddy-intl" | "zcode" | "qoder-intl" => Err(PlatformError::Local(
            "该平台仅支持扫描本机当前账号：请在对应官方客户端登录后使用「导入」".into(),
        )),
        "trae" => {
            let mut l = crate::adapters::trae::start_login();
            let listener = bind_ports(&[l.port, 17388]).await?;
            let port = listener.local_addr().map_err(|e| PlatformError::Local(e.to_string()))?.port();
            l.port = port;
            // 端口可能与默认不同：重拼授权 URL
            let url = crate::adapters::trae::rebuild_login_url(&l);
            let shared: Arc<Mutex<Option<Result<Cred, String>>>> = Arc::new(Mutex::new(None));
            let sid = new_id();
            flows().lock().unwrap().insert(sid.clone(), Flow {
                session_id: sid.clone(), provider: "trae".into(), url: url.clone(),
                expires_at: now_ms() + 300_000,
                data: FlowData::Trae { shared: shared.clone() },
            });
            tokio::spawn(trae_wait_task(sid.clone(), listener, l, shared));
            Ok((sid, url, 300))
        }
        "minimax" => {
            let st = crate::adapters::minimax::start_device_login().await?;
            let sid = new_id();
            let url = st.verification_uri.clone();
            flows().lock().unwrap().insert(sid.clone(), Flow {
                session_id: sid.clone(), provider: "minimax".into(), url: url.clone(),
                expires_at: now_ms() + 300_000,
                data: FlowData::Mini { verifier: st.verifier, device_code: st.device_code, interval_secs: st.interval_secs.max(3) },
            });
            Ok((sid, url, 300))
        }
        "qoder" => {
            let verifier = rand_b64url(32);
            let nonce = new_id();
            let challenge = B64URL.encode(Sha256::digest(verifier.as_bytes()));
            let enc = crate::adapters::trae::urlencoding_lite;
            let url = format!("https://qoder.cn/device/selectAccounts?challenge={}&challenge_method=S256&nonce={}&machine_id={}", enc(&challenge), enc(&nonce), new_id());
            let sid = new_id();
            flows().lock().unwrap().insert(sid.clone(), Flow {
                session_id: sid.clone(), provider: "qoder".into(), url: url.clone(),
                expires_at: now_ms() + 300_000,
                data: FlowData::Qoder { verifier, nonce },
            });
            Ok((sid, url, 300))
        }
        "workbuddy" => {
            let v = platform_fetch("https://www.codebuddy.cn/v2/plugin/auth/state?platform=WorkBuddy", reqwest::Method::POST,
                Some(json!({})), &[("Content-Type", "application/json".into()), ("X-No-Authorization", "true".into()), ("X-No-User-Id", "true".into())]).await?;
            let d = crate::net::http::business(v)?;
            let state = d.get("state").and_then(|s| s.as_str()).ok_or_else(|| PlatformError::Format("平台未返回 state".into()))?.to_string();
            let auth_url = d.get("authUrl").and_then(|s| s.as_str()).ok_or_else(|| PlatformError::Format("平台未返回授权页面".into()))?.to_string();
            let allowed = ["https://www.workbuddy.cn/", "https://copilot.tencent.com/", "https://www.codebuddy.cn/"];
            if !allowed.iter().any(|a| auth_url.starts_with(a)) {
                return Err(PlatformError::Network("授权页面不在允许范围".into()));
            }
            let sid = new_id();
            flows().lock().unwrap().insert(sid.clone(), Flow {
                session_id: sid.clone(), provider: "workbuddy".into(), url: auth_url.clone(),
                expires_at: now_ms() + 300_000,
                data: FlowData::Wb { state },
            });
            Ok((sid, auth_url, 300))
        }
        p if p == "codearts" || p == "codearts-desktop" => {
            let client: &'static str = if p == "codearts-desktop" { "desktop" } else { "agent" };
            let verifier = hex_string(32);
            let private_jwk = new_p256_jwk();
            let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(|e| PlatformError::Local(format!("回调端口绑定失败: {e}")))?;
            let port = listener.local_addr().map_err(|e| PlatformError::Local(e.to_string()))?.port();
            if port < 10000 { return Err(PlatformError::Local("登录回调端口不可用，请重试".into())); }
            let ch = B64URL.encode(Sha256::digest(verifier.as_bytes()));
            let url = format!(
                "https://codearts.huaweicloud.com/portal/authorize?theme={theme}&locale=zh-cn&uri_scheme={scheme}&client_id={cid}&port={port}&code_challenge={ch}&code_challenge_method=SHA-256&ticket_id={ticket}&plugin-name=snap_AIIDE&plugin-version={pv}",
                theme = if client == "desktop" { "2" } else { "1" },
                scheme = if client == "desktop" { "codearts" } else { "codearts-agent" },
                cid = if client == "desktop" { "codearts" } else { "codearts-agent" },
                ticket = hex_string(32),
                pv = if client == "desktop" { "5.1.0" } else { "26.9.902" },
            );
            let shared: Arc<Mutex<Option<Result<Cred, String>>>> = Arc::new(Mutex::new(None));
            let sid = new_id();
            flows().lock().unwrap().insert(sid.clone(), Flow {
                session_id: sid.clone(), provider: p.to_string(), url: url.clone(),
                expires_at: now_ms() + 300_000,
                data: FlowData::CodeArts { shared: shared.clone() },
            });
            tokio::spawn(codearts_wait_task(sid.clone(), listener, verifier, private_jwk, client, port, shared));
            Ok((sid, url, 300))
        }
        other => Err(PlatformError::Local(format!("platform {other} 不支持登录"))),
    }
}

async fn bind_ports(ports: &[u16]) -> Result<TcpListener, PlatformError> {
    for p in ports {
        if let Ok(l) = TcpListener::bind(("127.0.0.1", *p)).await {
            return Ok(l);
        }
    }
    TcpListener::bind(("127.0.0.1", 0)).await.map_err(|e| PlatformError::Local(format!("回调端口绑定失败: {e}")))
}

fn new_p256_jwk() -> Value {
    let sk = p256::SecretKey::random(&mut rand::rngs::OsRng);
    let skb: [u8; 32] = sk.to_bytes().into();
    let vk = p256::ecdsa::SigningKey::from(&sk);
    let _ = &vk; // keep type alive
    let vk_bytes = vk.to_bytes();
    // vk.to_bytes() 是 32 字节压缩标量（sec1 非这里所需）——改用 EncodePoint
    let pt = vk.verifying_key().to_encoded_point(false);
    let _ = vk_bytes;
    let x = B64URL.encode(pt.x().unwrap().as_slice());
    let y = B64URL.encode(pt.y().unwrap().as_slice());
    let d = B64URL.encode(skb);
    json!({ "kty": "EC", "crv": "P-256", "x": x, "y": y, "d": d })
}

// ===== Trae 后台回调任务 =====

async fn trae_wait_task(sid: String, listener: TcpListener, login: crate::adapters::trae::TraeLogin, _shared: Arc<Mutex<Option<Result<Cred, String>>>>) {
    let res: Result<Cred, String> = async {
        // 授权页在真正回调前可能先打一发不含 authCode 的请求（OPTIONS 预检、
        // 监听探活、中转跳转等）——逐个消费，直到出现 authCode 或显式 error，
        // 总预算约 5 分钟。之前首个请求即被消费，导致 "missing authCode"。
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(299);
        let params = loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() { return Err("登录回调等待超时（5 分钟未完成）".into()); }
            let query = wait_callback(&listener, remaining, "/authorize", "授权完成，请返回应用。").await.map_err(|e| e.to_string())?;
            let p = parse_query(&query);
            if p.contains_key("authCode") { break p; }
            if p.get("error").map(|s| !s.is_empty()).unwrap_or(false) {
                return Err("TRAE 授权未完成或已取消，请重新发起".into());
            }
        };
        if let Some(tid) = params.get("loginTraceID") {
            if *tid != login.trace_id { return Err("回调 traceId 不匹配（CSRF 防护），请重新发起".into()); }
        }
        let exchange = crate::adapters::trae::TraeLogin {
            trace_id: login.trace_id.clone(),
            verifier: login.verifier.clone(),
            port: login.port,
            identity: login.identity.clone(),
            url: String::new(),
        };
        crate::adapters::trae::exchange_auth_code(&exchange, &params).await.map_err(|e| e.to_string())
    }.await;
    if let Ok(mut m) = flows().lock() {
        if let Some(f) = m.get_mut(&sid) {
            f.data = FlowData::Done(res);
        }
    }
}

// ===== CodeArts 后台回调任务 =====

async fn codearts_wait_task(
    sid: String,
    listener: TcpListener,
    verifier: String,
    private_jwk: Value,
    client: &'static str,
    port: u16,
    _shared: Arc<Mutex<Option<Result<Cred, String>>>>,
) {
    let res: Result<Cred, String> = async {
        // 授权页可能先发一次 307 中转请求（带 secret/redirect 无 code），真正回调带 code
        let mut query = String::new();
        for _ in 0..3 {
            let q = wait_callback(&listener, std::time::Duration::from_secs(299), "/oauth/callback", "已收到授权，请返回 AI Free Router。").await.map_err(|e| e.to_string())?;
            let p = parse_query(&q);
            if p.contains_key("secret") && !p.contains_key("code") {
                continue;
            }
            query = q;
            break;
        }
        if query.is_empty() { return Err("码道授权未完成，请重新发起".into()); }
        let p = parse_query(&query);
        let code = p.get("code").ok_or_else(|| "回调里没有 code".to_string())?.clone();
        let url = format!("{}/v1/oauth2/tokens", crate::adapters::codearts::STS_BASE);
        let dpop = crate::adapters::codearts::dpop_header(&private_jwk, &url).map_err(|e| e.to_string())?;
        let pairs = [
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("code_verifier", verifier.as_str()),
            ("redirect_uri", &format!("http://127.0.0.1:{port}/oauth/callback")),
            ("client_id", if client == "desktop" { "codearts" } else { "codearts-agent" }),
        ];
        let body = pairs.iter().map(|(k, v)| format!("{}={}", crate::adapters::trae::urlencoding_lite(k), crate::adapters::trae::urlencoding_lite(v))).collect::<Vec<_>>().join("&");
        let http = reqwest::Client::builder().timeout(std::time::Duration::from_secs(15)).build().unwrap();
        let resp = http.post(&url).header("Content-Type", "application/x-www-form-urlencoded").header("DPoP", dpop).body(body)
            .send().await.map_err(|e| e.to_string())?;
        let v: Value = resp.json().await.map_err(|e| e.to_string())?;
        let d = if v.get("credentials").is_some() { v.clone() } else { v.get("result").cloned().unwrap_or(v.clone()) };
        let cred = d.get("credentials").ok_or_else(|| "码道授权已失效或登录未完成，请重新登录".to_string())?;
        let ak = cred.get("access_key_id").and_then(|a| a.as_str()).unwrap_or_default().to_string();
        let sk_ = cred.get("secret_access_key").and_then(|a| a.as_str()).unwrap_or_default().to_string();
        let st = cred.get("security_token").and_then(|a| a.as_str()).unwrap_or_default().to_string();
        if ak.is_empty() || sk_.is_empty() || st.is_empty() { return Err("码道授权已失效或登录未完成，请重新登录".into()); }
        let mut c = Cred {
            provider: if client == "desktop" { "codearts-desktop".into() } else { "codearts".into() },
            uid: String::new(),
            name: String::new(),
            token: String::new(),
            refresh_token: d.get("refresh_token").and_then(|r| r.as_str()).map(|s| s.to_string()),
            expires_at: cred.get("expiration").and_then(|e| e.as_str()).map(|s| s.to_string())
                .or_else(|| {
                    // 官方：无 expiration 时按当前时间 +1h
                    let exp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() + 3600;
                    Some(format!("{exp}"))
                }),
            meta: json!({
                "access_key_id": ak, "secret_access_key": sk_, "security_token": st,
                "privateJwk": private_jwk, "refresh_token": d.get("refresh_token").and_then(|r| r.as_str()).unwrap_or(""),
                "verifier": verifier, "client": client,
            }),
        };
        // caller-identity 拿 uid/name（走 Adapter verify）：字段规范化与官方客户端一致
        let adapter = crate::adapters::codearts::CodeArtsAdapter { desktop: client == "desktop" };
        let id = crate::adapters::Adapter::verify(&adapter, &c).await.map_err(|e| e.to_string())?;
        let (pid, pname, domain) = crate::adapters::codearts::identity_of(&id);
        if pid.is_empty() {
            return Err("码道未返回账号标识".into());
        }
        c.uid = pid;
        c.name = if pname.is_empty() { "华为云码道".to_string() } else { pname };
        if !domain.is_empty() {
            c.meta["domain_id"] = json!(domain);
        }
        Ok(c)
    }.await;
    if let Ok(mut m) = flows().lock() {
        if let Some(f) = m.get_mut(&sid) {
            f.data = FlowData::Done(res);
        }
    }
}

// ===== 轮询 =====

/// 轮询结果：status = pending | ready | expired；ready 时携带凭据。
pub struct PollOut { pub status: String, pub credential: Option<Cred> }

/// 查询登录进度：trae/codearts 读后台回调任务结果，minimax/qoder/workbuddy 现场轮询换票；
/// 拿到凭据即移除会话，会话过期返回 expired。
pub async fn poll_login(session_id: &str) -> Result<PollOut, PlatformError> {
    let f = {
        let m = flows().lock().unwrap();
        match m.get(session_id) {
            Some(f) if now_ms() > f.expires_at => { drop(m); flows().lock().unwrap().remove(session_id); None }
            Some(f) => Some(f.clone()),
            None => None,
        }
    };
    let Some(f) = f else {
        return Ok(PollOut { status: "expired".into(), credential: None });
    };
    let cred = match f.data {
        FlowData::Done(res) => match res {
            Ok(c) => Some(c),
            Err(e) => { flows().lock().unwrap().remove(session_id); return Err(PlatformError::Local(e)); }
        },
        FlowData::Mini { verifier, device_code, interval_secs } => {
            let st = crate::adapters::minimax::MiniStart { device_code, verification_uri: String::new(), interval_secs, verifier };
            match crate::adapters::minimax::poll_device_login(&st).await? {
                crate::adapters::minimax::MiniPoll::Pending { interval_secs: _ } => None,
                #[allow(unused_variables)]
                crate::adapters::minimax::MiniPoll::Ready(at_v, tok) => {
                    let at = at_v.as_str().unwrap_or_default().to_string();
                    let prof = minimax_profile(&at).await?;
                    Some(Cred {
                        provider: "minimax".into(),
                        uid: prof.0,
                        name: prof.1,
                        token: at,
                        refresh_token: tok.get("refresh_token").and_then(|r| r.as_str()).map(|s| s.to_string()),
                        expires_at: Some(iso_from_ms(now_ms() + tok.get("expires_in").and_then(|e| e.as_u64()).unwrap_or(3600) * 1000)),
                        meta: json!({}),
                    })
                }
            }
        }
        FlowData::Qoder { verifier, nonce } => {
            let enc = crate::adapters::trae::urlencoding_lite;
            let url = format!("https://openapi.qoder.com.cn/api/v1/deviceToken/poll?nonce={}&verifier={}&challenge_method=S256", enc(&nonce), enc(&verifier));
            let code = raw_get_status(&url).await?;
            if code == 202 || code == 404 { None }
            else if !(200..300).contains(&code) { return Err(PlatformError::Local("Qoder 授权未完成，请重新发起".into())); }
            else {
                let s = platform_fetch(&url, reqwest::Method::GET, None, &[]).await?;
                let token = s.get("token").and_then(|t| t.as_str()).ok_or_else(|| PlatformError::Format("授权响应缺少令牌".into()))?.to_string();
                let p = crate::net::http::business(platform_fetch("https://openapi.qoder.com.cn/api/v1/userinfo",
                    reqwest::Method::GET, None, &[("Authorization", format!("Bearer {token}")), ("Cosy-ClientType", "10".into())]).await?)?;
                let uid = value_to_string(p.get("id").or_else(|| p.get("userId")).or_else(|| p.get("user_id"))
                    .ok_or_else(|| PlatformError::Format("授权缺少账号标识".into()))?);
                let name = p.get("name").or_else(|| p.get("nickname")).and_then(|n| n.as_str()).unwrap_or("Qoder 账号").to_string();
                Some(Cred { provider: "qoder".into(), uid, name, token, refresh_token: None,
                    expires_at: s.get("expires_at").and_then(|e| e.as_str()).map(|x| x.to_string()), meta: json!({}) })
            }
        }
        FlowData::Wb { state } => {
            let enc = crate::adapters::trae::urlencoding_lite;
            let url = format!("https://www.codebuddy.cn/v2/plugin/auth/token?state={}", enc(&state));
            let v = platform_fetch(&url, reqwest::Method::GET, None, &[("X-No-Authorization", "true".into())]).await?;
            if v.get("code").and_then(|c| c.as_i64()) == Some(11217) { None }
            else {
                let s = crate::net::http::business(v)?;
                let access = s.get("accessToken").and_then(|a| a.as_str()).ok_or_else(|| PlatformError::Format("授权响应缺少令牌".into()))?.to_string();
                let info = platform_fetch(&format!("https://www.codebuddy.cn/v2/plugin/login/account?state={}", enc(&state)),
                    reqwest::Method::GET, None, &[("Authorization", format!("Bearer {access}")), ("X-No-User-Id", "true".into())]).await?;
                if info.get("code").and_then(|c| c.as_i64()) == Some(12151) { None }
                else {
                    let acc = crate::net::http::business(info)?;
                    let uid = acc.get("uid").map(value_to_string)
                        .ok_or_else(|| PlatformError::Format("授权无法确认身份".into()))?;
                    let name = acc.get("nickname").or_else(|| acc.get("name")).and_then(|n| n.as_str()).unwrap_or("WorkBuddy 账号").to_string();
                    Some(Cred {
                        provider: "workbuddy".into(), uid, name,
                        token: access.clone(),
                        refresh_token: s.get("refreshToken").and_then(|r| r.as_str()).map(|x| x.to_string()),
                        expires_at: jwt_exp(&access).map(iso_from_ms),
                        meta: json!({
                            "domain": s.get("domain").and_then(|d| d.as_str()).unwrap_or("www.codebuddy.cn"),
                            "enterpriseId": acc.get("enterpriseId").cloned().unwrap_or(Value::Null),
                        }),
                    })
                }
            }
        }
        FlowData::Trae { .. } | FlowData::CodeArts { .. } => None,
    };
    let status = if cred.is_some() { "ready" } else { "pending" };
    if cred.is_some() { flows().lock().unwrap().remove(session_id); }
    Ok(PollOut { status: status.into(), credential: cred })
}

// ===== 工具 =====

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string().trim_matches('"').to_string(),
    }
}

async fn minimax_profile(token: &str) -> Result<(String, String), PlatformError> {
    let v = platform_fetch("https://agent.minimax.cn/matrix/api/v1/user/profile",
        reqwest::Method::GET, None, &[("Authorization", format!("Bearer {token}"))]).await?;
    let d = crate::net::http::business(v)?;
    let uid = d.get("user_id").map(value_to_string)
        .ok_or_else(|| PlatformError::Format("MiniMax 未返回账号标识".into()))?;
    let name = d.get("user_name").and_then(|n| n.as_str()).unwrap_or("MiniMax Code").to_string();
    Ok((uid, name))
}

async fn raw_get_status(url: &str) -> Result<u16, PlatformError> {
    let c = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(std::time::Duration::from_secs(15)).build().unwrap();
    let r = c.get(url).send().await.map_err(|e| PlatformError::Network(e.to_string()))?;
    Ok(r.status().as_u16())
}

fn jwt_exp(token: &str) -> Option<u64> {
    let p = token.split('.').nth(1)?;
    let b = B64URL.decode(p).ok()?;
    let v: Value = serde_json::from_slice(&b).ok()?;
    v.get("exp").and_then(|e| e.as_u64())
}

fn iso_from_ms(ms: u64) -> String {
    let secs = ms / 1000;
    let days = secs / 86400;
    let rem = secs % 86400;
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z", rem / 3600, (rem % 3600) / 60, rem % 60)
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}