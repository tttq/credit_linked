//! 码道并发语义实测探针（诊断用）：并发发 N 个 chat，观测上游行为。
//!
//! 用法：cargo run -p credit-gateway --example caprobe -- [并发数] [模式]
//!   模式 unique = 每请求唯一 user-session-id（当前网关实现）
//!   模式 fixed  = 所有请求共用同一个 user-session-id
//!
//! 观测点：
//!   ① TM.00001041（并发会话数上限）何时出现
//!   ② 同 session 并发 turn 是否「200 后掐流」（error decoding response body）
//!   ③ 流中途断开的比例
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use credit_core::adapters::Cred;
use credit_core::db::Store;
use serde_json::json;

fn read_cred(provider: &str) -> Option<Cred> {
    let store = Store::open_default().ok()?;
    let rows = store.list_accounts().ok()?;
    rows.iter()
        .filter(|(p, _, _, _, _)| p == provider)
        .filter_map(|(_, _, _, _, auth)| serde_json::from_str::<Cred>(auth).ok())
        .next()
}

/// 优先用**本机客户端的实时会话**（DB 里的 STS 往往已过期 → APIG.0301），
/// 退化到 DB 凭据。
fn live_cred(provider: &str) -> Option<Cred> {
    let live = credit_core::adapters::codearts_local::read_live_session(true)
        .or_else(|| credit_core::adapters::codearts_local::read_live_session(false))?;
    let s = &live.session;
    let get = |p: &str| s.pointer(p).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let mut c = read_cred(provider)?;
    c.meta["access_key_id"] = json!(get("/session/accessKey"));
    c.meta["secret_access_key"] = json!(get("/session/secretKey"));
    c.meta["security_token"] = json!(get("/session/securityToken"));
    Some(c)
}

/// 直接复刻 upstream::codearts_chat_stream 的签名+发请求逻辑，但允许指定 session 模式，
/// 并把「流中途断开」与「HTTP 错误」区分统计。
async fn one_chat(
    cred: &Cred,
    idx: usize,
    mode: &str,
    fixed_sid: &str,
    model: &str,
) -> Result<(usize, String), (usize, String)> {
    let sts = cred.meta.get("security_token").and_then(|t| t.as_str()).unwrap_or("");
    let ak = cred.meta.get("access_key_id").and_then(|a| a.as_str()).unwrap_or("");
    let sk = cred.meta.get("secret_access_key").and_then(|a| a.as_str()).unwrap_or("");
    let host = "snap-access.cn-north-4.myhuaweicloud.com";
    let path = "/api/v2/chat/completions";
    let body = credit_gateway::protocol::codearts_chat_body(
        &[json!({"role":"user","content":"say hi"})],
        "",
        model,
        Some(64),
        None,
    );
    let body_str = serde_json::to_string(&body).unwrap_or_default();

    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
    let (y, mo, d) = credit_core::adapters::civil_from_days(now / 86400);
    let rem = now % 86400;
    let created = format!("{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, (rem % 60) / 60, rem % 60);

    let sid = if mode == "fixed" { fixed_sid.to_string() } else { uuid::Uuid::new_v4().to_string() };
    let extra: Vec<(String, String)> = vec![
        ("x-security-token".into(), sts.to_string()),
        ("x-auth-token".into(), sts.to_string()),
        ("user-session-id".into(), sid.clone()),
        ("x-snap-traceid".into(), uuid::Uuid::new_v4().simple().to_string()),
        ("x-span-id".into(), uuid::Uuid::new_v4().to_string()),
        ("traceid".into(), uuid::Uuid::new_v4().simple().to_string()),
        ("x-ot-trace-id".into(), uuid::Uuid::new_v4().simple().to_string()),
        ("x-ot-span-id".into(), uuid::Uuid::new_v4().simple().to_string()),
        ("x-ot-client-version".into(), "0.2.9".into()),
        ("x-ot-client-type".into(), "desktop".into()),
        ("ide-name".into(), "codearts_space".into()),
        ("ide-version".into(), "0.2.9".into()),
        ("plugin-name".into(), "codearts_space".into()),
        ("plugin-version".into(), "0.2.9".into()),
        ("user-msg-id".into(), format!("msg_{}", uuid::Uuid::new_v4().simple())),
        ("created-time".into(), created),
        ("model-id".into(), model.to_string()),
        ("model-name".into(), model.to_string()),
    ];
    let (auth, signed) = credit_core::adapters::codearts::sign_codearts(
        ak, sk, "POST", host, path, "", &extra, &body_str, Some(sts),
    );
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .read_timeout(std::time::Duration::from_secs(60))
        .build()
        .unwrap();
    let url = format!("https://{host}{path}");
    let mut req = client.post(&url).header("Content-Type", "application/json");
    for (k, v) in &signed {
        req = req.header(k, v);
    }
    let resp = match req.header("Authorization", &auth).body(body_str).send().await {
        Ok(r) => r,
        Err(e) => return Err((idx, format!("network: {e}"))),
    };
    let status = resp.status().as_u16();
    if !(200..300).contains(&status) {
        let text = resp.text().await.unwrap_or_default();
        return Err((idx, format!("HTTP {status}: {}", text.chars().take(220).collect::<String>())));
    }
    // 200 → 读流，统计是否中途断开
    use futures_util::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut got = 0usize;
    let mut raw = Vec::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(b) => { got += 1; if raw.len() < 4000 { raw.extend_from_slice(&b); } }
            Err(e) => {
                return Err((idx, format!("STREAM-BREAK after {got} chunks: {e}")));
            }
        }
    }
    let body_txt = String::from_utf8_lossy(&raw).chars().take(300).collect::<String>();
    Ok((idx, format!("OK {got} chunks | body={}", body_txt)))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let n: usize = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(4);
    let mode = std::env::args().nth(2).unwrap_or_else(|| "unique".into());
    let model = std::env::args().nth(3).unwrap_or_else(|| "glm-5.3-flash".into());

    // "cpool" 模式：并发 N 个请求走网关真实路径（含会话池），验证高并发不超上游会话上限。
    if mode == "cpool" {
        let cred = Arc::new(live_cred("codearts").or_else(|| live_cred("codearts-desktop"))
            .or_else(|| read_cred("codearts"))
            .ok_or_else(|| anyhow::anyhow!("no codearts cred"))?);
        let model = std::env::args().nth(3).unwrap_or_else(|| "glm-5.3-flash".into());
        println!("cpool mode: concurrency={n} model={model}");
        let okc = Arc::new(AtomicUsize::new(0));
        let errc = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for i in 0..n {
            let cred = cred.clone();
            let model = model.clone();
            let okc = okc.clone();
            let errc = errc.clone();
            tasks.push(tokio::spawn(async move {
                let body = credit_gateway::protocol::codearts_chat_body(
                    &[json!({"role":"user","content":"say hi"})], "", &model, Some(64), None,
                );
                match credit_gateway::upstream::codearts_chat_stream(&cred, body, true).await {
                    Ok(mut stream) => {
                        use futures_util::StreamExt;
                        let mut txt = String::new();
                        let mut e = None;
                        while let Some(item) = stream.next().await {
                            match item {
                                Ok(credit_gateway::upstream::StreamEvent::Text(t)) => txt.push_str(&t),
                                Ok(credit_gateway::upstream::StreamEvent::Error(m, _)) => e = Some(m),
                                Err(m) => e = Some(m),
                                _ => {}
                            }
                        }
                        match e {
                            Some(m) => { errc.fetch_add(1, Ordering::SeqCst); println!("[{i}] ERR: {}", m.chars().take(120).collect::<String>()); }
                            None => { okc.fetch_add(1, Ordering::SeqCst); println!("[{i}] OK len={}", txt.len()); }
                        }
                    }
                    Err(m) => { errc.fetch_add(1, Ordering::SeqCst); println!("[{i}] ERR: {}", m.chars().take(120).collect::<String>()); }
                }
            }));
        }
        for t in tasks { let _ = t.await; }
        println!("=== cpool summary: ok={} err={} ===", okc.load(Ordering::SeqCst), errc.load(Ordering::SeqCst));
        return Ok(());
    }

    // "pool" 模式：走**网关真实代码路径** upstream::codearts_chat_stream（含会话池），
    // 连续/并发发 N 次，验证不会耗尽上游会话配额（TM.00001041）。
    if mode == "pool" {
        let cred = live_cred("codearts").or_else(|| live_cred("codearts-desktop"))
            .or_else(|| read_cred("codearts"))
            .ok_or_else(|| anyhow::anyhow!("no codearts cred"))?;
        let model = std::env::args().nth(3).unwrap_or_else(|| "glm-5.3-flash".into());
        println!("pool mode: total={n} model={model}");
        let mut okc = 0usize;
        let mut errc = 0usize;
        for i in 0..n {
            let body = credit_gateway::protocol::codearts_chat_body(
                &[json!({"role":"user","content":"say hi"})], "", &model, Some(64), None,
            );
            match credit_gateway::upstream::codearts_chat_stream(&cred, body, true).await {
                Ok(mut stream) => {
                    use futures_util::StreamExt;
                    let mut txt = String::new();
                    let mut e = None;
                    while let Some(item) = stream.next().await {
                        match item {
                            Ok(credit_gateway::upstream::StreamEvent::Text(t)) => txt.push_str(&t),
                            Ok(credit_gateway::upstream::StreamEvent::Error(m, _)) => e = Some(m),
                            Err(m) => e = Some(m),
                            _ => {}
                        }
                    }
                    match e {
                        Some(m) => { errc += 1; println!("[{i}] ERR: {}", m.chars().take(140).collect::<String>()); }
                        None => { okc += 1; println!("[{i}] OK len={}", txt.len()); }
                    }
                }
                Err(m) => { errc += 1; println!("[{i}] ERR: {}", m.chars().take(140).collect::<String>()); }
            }
        }
        println!("=== pool summary: ok={okc} err={errc} ===");
        return Ok(());
    }

    // "models" 模式：列出可用模型（用实时会话凭据）
    if mode == "models" {
        let cred = live_cred("codearts").or_else(|| live_cred("codearts-desktop"))
            .ok_or_else(|| anyhow::anyhow!("no codearts cred"))?;
        match credit_gateway::upstream::codearts_fetch_models(&cred, true).await {
            Ok(ms) => {
                println!("models OK count={}", ms.len());
                for x in ms { println!("  id={} name={} ctx={:?}", x.id, x.name, x.context_window); }
            }
            Err(e) => println!("models ERR: {e}"),
        }
        return Ok(());
    }

    // "reuse" 模式：同一个 session id 顺序发 N 次，验证会话是否可复用（不新增上游会话）
    if mode == "reuse" {
        let cred = live_cred("codearts").or_else(|| live_cred("codearts-desktop"))
            .or_else(|| read_cred("codearts"))
            .ok_or_else(|| anyhow::anyhow!("no codearts cred"))?;
        let model = std::env::args().nth(3).unwrap_or_else(|| "glm-5.3-flash".into());
        let sid = std::env::args().nth(4).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        println!("reuse sid={sid} model={model}");
        for i in 0..n {
            match one_chat(&cred, i, "fixed", &sid, &model).await {
                Ok((_, msg)) => println!("[{i}] OK: {}", msg.chars().take(120).collect::<String>()),
                Err((_, msg)) => println!("[{i}] ERR: {}", msg.chars().take(160).collect::<String>()),
            }
        }
        return Ok(());
    }

    let cred = live_cred("codearts")
        .or_else(|| live_cred("codearts-desktop"))
        .or_else(|| read_cred("codearts"))
        .or_else(|| read_cred("codearts-desktop"))
        .ok_or_else(|| anyhow::anyhow!("no codearts cred"))?;
    println!("mode={mode} concurrency={n}");

    let fixed_sid = uuid::Uuid::new_v4().to_string();
    let cred = Arc::new(cred);
    let fixed = Arc::new(fixed_sid.clone());
    let mode2 = mode.clone();

    let ok_count = Arc::new(AtomicUsize::new(0));
    let brk_count = Arc::new(AtomicUsize::new(0));
    let http_err = Arc::new(AtomicUsize::new(0));

    let mut tasks = Vec::new();
    for i in 0..n {
        let c = cred.clone();
        let f = fixed.clone();
        let m = mode2.clone();
        let mm = model.clone();
        let okc = ok_count.clone();
        let brkc = brk_count.clone();
        let hec = http_err.clone();
        tasks.push(tokio::spawn(async move {
            match one_chat(&c, i, &m, &f, &mm).await {
                Ok((_, msg)) => { okc.fetch_add(1, Ordering::SeqCst); println!("[{i}] OK: {msg}"); }
                Err((_, msg)) => {
                    if msg.contains("STREAM-BREAK") { brkc.fetch_add(1, Ordering::SeqCst); }
                    else { hec.fetch_add(1, Ordering::SeqCst); }
                    println!("[{i}] ERR: {msg}");
                }
            }
        }));
    }
    for t in tasks { let _ = t.await; }
    println!("=== summary: ok={} stream_break={} http_err={} ===",
        ok_count.load(Ordering::SeqCst),
        brk_count.load(Ordering::SeqCst),
        http_err.load(Ordering::SeqCst));
    Ok(())
}
