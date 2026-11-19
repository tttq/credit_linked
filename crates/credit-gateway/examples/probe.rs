//! 诊断探针：读取本机真实凭据，逐个验证各平台最小请求链路。
//! 用法：cargo run -p credit-gateway --example probe -- <cmd>
//!   dump            列出全部账号（打印 cred.meta 键）
//!   ca-session      解密本机码道会话，打印字段与过期时间
//!   ca-verify       caller-identity
//!   ca-models       /v1/model/builtin
//!   ca-quota        /snap-manager/v1/statistics/plugin
//!   ca-refresh      用本机 RT 走 STS refresh（会轮换 RT，必要时把新 RT 写回本地客户端文件）
//!   mm-chat         用 DB 里的 MiniMax 凭据发一次带 tool_result 的 anthropic 请求
//!   qoder-chat      用 DB 里的 Qoder 凭据走 sidecar 签名发一次 chat
use credit_core::adapters::codearts::CodeArtsAdapter;
use credit_core::adapters::{Adapter, Cred};
use credit_core::db::Store;
use serde_json::{json, Value};

fn read_cred(provider: &str) -> Option<Cred> {
    let store = Store::open_default().ok()?;
    let rows = store.list_accounts().ok()?;
    rows.iter()
        .filter(|(p, _, _, _, _)| p == provider)
        .filter_map(|(_, _, _, _, auth)| serde_json::from_str::<Cred>(auth).ok())
        .next()
}

fn trunc(s: &str) -> String {
    if s.len() <= 12 { format!("<{}>", s.len()) } else { format!("{}…({}B)", &s[..8], s.len()) }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cmd = std::env::args().nth(1).unwrap_or_else(|| "dump".into());
    match cmd.as_str() {
        "dump" => {
            let store = Store::open_default()?;
            for (p, u, n, _s, auth) in store.list_accounts()? {
                let cred = serde_json::from_str::<Cred>(&auth).ok();
                let meta_keys = cred.as_ref()
                    .map(|c| c.meta.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()).unwrap_or_default())
                    .unwrap_or_default();
                let fp = |s: &str| -> String {
                    if s.is_empty() { "-".into() } else {
                        use sha2::Digest;
                        let h = sha2::Sha256::digest(s.as_bytes());
                        h.iter().take(6).map(|b| format!("{b:02x}")).collect()
                    }
                };
                let (tok, rt, exp) = cred.as_ref()
                    .map(|c| (fp(&c.token), c.refresh_token.as_deref().map(fp).unwrap_or_else(|| "-".into()), c.expires_at.clone().unwrap_or_default()))
                    .unwrap_or_default();
                println!("{p}\t{u}\t{n}\tauth_len={}\ttok_fp={tok}\trt_fp={rt}\texpires={exp}\tmeta={:?}", auth.len(), meta_keys);
            }
        }
        "ca-session" => {
            let live = credit_core::adapters::codearts_local::read_live_session(true)
                .or_else(|| credit_core::adapters::codearts_local::read_live_session(false));
            match live {
                Some(l) => {
                    let s = &l.session;
                    println!("desktop={}", l.desktop);
                    for path in ["/session/userId", "/session/userName", "/session/accessKey", "/session/secretKey", "/session/securityToken", "/session/refreshToken", "/session/expiresAt", "/loginContext/pkcePair/codeVerifier", "/loginContext/dpopKeyPair/privateKey"] {
                        let v = s.pointer(path).cloned().unwrap_or(Value::Null);
                        let shown = match &v {
                            Value::Null => "NULL".to_string(),
                            Value::String(x) => format!("len={} head={}", x.len(), x.chars().take(16).collect::<String>()),
                            other => other.to_string().chars().take(60).collect(),
                        };
                        println!("  {path} = {shown}");
                    }
                    if let Some(ms) = s.pointer("/session/expiresAt").and_then(|e| e.as_i64()) {
                        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
                        println!("  expiresAt={ms} now={now} remain_min={}", (ms - now) / 60000);
                    }
                    // 与 DB 凭据对比指纹
                    use sha2::Digest;
                    let fp = |x: &str| -> String {
                        if x.is_empty() { "-".into() } else {
                            let h = sha2::Sha256::digest(x.as_bytes());
                            h.iter().take(6).map(|b| format!("{b:02x}")).collect()
                        }
                    };
                    let db = read_cred("codearts").or_else(|| read_cred("codearts-desktop"));
                    let s_rt = s.pointer("/session/refreshToken").and_then(|v| v.as_str()).unwrap_or("");
                    let s_st = s.pointer("/session/securityToken").and_then(|v| v.as_str()).unwrap_or("");
                    println!("  fp refreshToken={} securityToken={}", fp(s_rt), fp(s_st));
                    if let Some(db) = db {
                        let rt = db.refresh_token.as_deref().unwrap_or("");
                        let st = db.meta.get("security_token").and_then(|v| v.as_str()).unwrap_or("");
                        println!("  db fp refreshToken={} securityToken={}", fp(rt), fp(st));
                        println!("  same_rt={} same_st={}", rt == s_rt, st == s_st);
                    }
                }
                None => println!("no codearts session found"),
            }
        }
        "ca-verify" => {
            let c = read_cred("codearts").or_else(|| read_cred("codearts-desktop")).ok_or_else(|| anyhow::anyhow!("no codearts cred"))?;
            let desktop = c.provider == "codearts-desktop";
            let a = CodeArtsAdapter { desktop };
            match a.verify(&c).await {
                Ok(v) => println!("verify OK: {}", serde_json::to_string_pretty(&v).unwrap_or_default()),
                Err(e) => println!("verify ERR: {e}"),
            }
        }
        "ca-models" => {
            let c = read_cred("codearts").or_else(|| read_cred("codearts-desktop")).ok_or_else(|| anyhow::anyhow!("no codearts cred"))?;
            let desktop = c.provider == "codearts-desktop";
            match credit_gateway::upstream::codearts_fetch_models(&c, desktop).await {
                Ok(ms) => {
                    println!("models OK count={}", ms.len());
                    for m in ms {
                        println!("  id={} name={} ctx={:?} max={:?} img={:?}", m.id, m.name, m.context_window, m.max_tokens, m.supports_images);
                    }
                }
                Err(e) => println!("models ERR: {e}"),
            }
        }
        "set-platform" => {
            let store = Store::open_default()?;
            let p = std::env::args().nth(2).unwrap_or_default();
            store.set_setting("gw_platform", &p)?;
            println!("gw_platform={}", store.get_setting("gw_platform", "")?);
            println!("gatewayStrict={}", store.get_setting("gatewayStrict", "")?);
        }
        "settings" => {
            let store = Store::open_default()?;
            for k in ["gw_platform", "gw_account", "gatewayStrict", "gatewayPort", "gatewayApiKey", "gatewayRpm", "gw_rules", "refreshMinutes"] {
                println!("{k} = {}", store.get_setting(k, "<unset>")?);
            }
        }
        "logs" => {
            let store = Store::open_default()?;
            for l in store.list_gateway_logs(60)? {
                println!("#{} {} {} {} ok={} status={:?} dur={:?} msg={}",
                    l.id, l.at, l.platform, l.model, l.ok, l.status_code, l.duration_ms,
                    l.message.unwrap_or_default());
            }
        }
        "ca-quota" => {
            let c = read_cred("codearts").or_else(|| read_cred("codearts-desktop")).ok_or_else(|| anyhow::anyhow!("no codearts cred"))?;
            let desktop = c.provider == "codearts-desktop";
            let a = CodeArtsAdapter { desktop };
            match a.quota(&c).await {
                Ok(q) => println!("quota OK: {}", serde_json::to_string(&q).unwrap_or_default()),
                Err(e) => println!("quota ERR: {e}"),
            }
        }
        "ca-refresh" => {
            let c = read_cred("codearts").or_else(|| read_cred("codearts-desktop")).ok_or_else(|| anyhow::anyhow!("no codearts cred"))?;
            let desktop = c.provider == "codearts-desktop";
            let a = CodeArtsAdapter { desktop };
            println!("refresh start; RT={} verifier={} jwk_present={}",
                c.refresh_token.as_deref().map(trunc).unwrap_or_else(|| "none".into()),
                c.meta.get("verifier").and_then(|v| v.as_str()).map(|s| s.len()).unwrap_or(0),
                c.meta.get("privateJwk").map(|v| !v.is_null()).unwrap_or(false));
            match a.refresh(&c).await {
                Ok(nc) => println!("refresh OK: new_token_len={} expires={:?} meta_keys={:?}",
                    nc.token.len(), nc.expires_at,
                    nc.meta.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()).unwrap_or_default()),
                Err(e) => println!("refresh ERR: {e}"),
            }
        }
        "ca-jwk" => {
            let c = read_cred("codearts").or_else(|| read_cred("codearts-desktop")).ok_or_else(|| anyhow::anyhow!("no codearts cred"))?;
            let jwk = c.meta.get("privateJwk").cloned().unwrap_or(Value::Null);
            println!("type={} value_head={}", match &jwk { Value::String(_) => "string", Value::Object(_) => "object", Value::Null => "null", _ => "other" }, jwk.to_string().chars().take(120).collect::<String>());
            if let Value::String(s) = &jwk {
                println!("parsed_string_head={}", s.chars().take(120).collect::<String>());
                if let Ok(v) = serde_json::from_str::<Value>(s) {
                    println!("inner_keys={:?}", v.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()).unwrap_or_default());
                }
            } else if let Some(o) = jwk.as_object() {
                println!("keys={:?}", o.keys().cloned().collect::<Vec<_>>());
            }
            if let Some(live) = credit_core::adapters::codearts_local::read_live_session(true).or_else(|| credit_core::adapters::codearts_local::read_live_session(false)) {
                let v = live.session.pointer("/loginContext/dpopKeyPair/privateKey").cloned().unwrap_or(Value::Null);
                println!("live type={} head={}", match &v { Value::String(_) => "string", Value::Object(_) => "object", Value::Null => "null", _ => "other" }, v.to_string().chars().take(120).collect::<String>());
            }
        }
        "ca-refresh-fake" => {
            // 用假 RT 验证 refresh 请求形态（DPoP/表单/字段），不会消费真实 RT
            let c = read_cred("codearts").or_else(|| read_cred("codearts-desktop")).ok_or_else(|| anyhow::anyhow!("no codearts cred"))?;
            let desktop = c.provider == "codearts-desktop";
            let mut fake = c.clone();
            fake.refresh_token = Some("invalid-refresh-token-for-probe".into());
            fake.meta["refresh_token"] = json!("invalid-refresh-token-for-probe");
            let a = CodeArtsAdapter { desktop };
            match a.refresh(&fake).await {
                Ok(_) => println!("refresh(fake) UNEXPECTED OK"),
                Err(e) => println!("refresh(fake) ERR（预期鉴权类错误）: {e}"),
            }
        }
        "mm-chat" => {
            let c = read_cred("minimax").ok_or_else(|| anyhow::anyhow!("no minimax cred"))?;
            // 模拟 dsh 工具回合：assistant(tool_calls) + tool(tool_call_id)
            let messages = vec![
                json!({"role": "user", "content": "查一下北京天气"}),
                json!({"role": "assistant", "content": "", "tool_calls": [{"id": "call_abc123", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":\"北京\"}"}}]}),
                json!({"role": "tool", "tool_call_id": "call_abc123", "content": "{\"temp\":18,\"cond\":\"晴\"}"}),
            ];
            let mut body = credit_gateway::protocol::minimax_chat_body(&messages, "你是天气助手", "MiniMax-M3.1-Flash", Some(256));
            credit_gateway::protocol::attach_anthropic_tools(&mut body, &[json!({"type":"function","function":{"name":"get_weather","description":"查天气","parameters":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}})], Some(&json!("auto")));
            println!("body messages: {}", serde_json::to_string_pretty(&body["messages"]).unwrap_or_default());
            match credit_gateway::upstream::minimax_chat_stream(&c, body).await {
                Ok(events) => {
                    use futures_util::StreamExt;
                    let mut ev = events;
                    let mut text = String::new();
                    let mut err = None;
                    while let Some(item) = ev.next().await {
                        match item {
                            Ok(credit_gateway::upstream::StreamEvent::Text(t)) => text.push_str(&t),
                            Ok(credit_gateway::upstream::StreamEvent::Error(m, _)) => err = Some(m),
                            Err(e) => err = Some(e),
                            _ => {}
                        }
                    }
                    println!("minimax stream text={:?} err={:?}", text, err);
                }
                Err(e) => println!("minimax chat ERR: {e}"),
            }
        }
        "mm-tools" => {
            let c = read_cred("minimax").ok_or_else(|| anyhow::anyhow!("no minimax cred"))?;
            let messages = vec![json!({"role": "user", "content": "北京天气怎么样？请调用 get_weather 工具查询"})];
            let mut body = credit_gateway::protocol::minimax_chat_body(&messages, "你是天气助手，必须调用工具", "MiniMax-M3.1-Flash", Some(256));
            credit_gateway::protocol::attach_anthropic_tools(&mut body, &[json!({"type":"function","function":{"name":"get_weather","description":"查天气","parameters":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}})], Some(&json!("auto")));
            match credit_gateway::upstream::minimax_chat_stream(&c, body).await {
                Ok(events) => {
                    use futures_util::StreamExt;
                    let mut ev = events;
                    while let Some(item) = ev.next().await {
                        match item {
                            Ok(credit_gateway::upstream::StreamEvent::ToolCall { index, id, name, arguments }) =>
                                println!("TOOL index={index} id={id:?} name={name:?} args={arguments:?}"),
                            Ok(credit_gateway::upstream::StreamEvent::Text(t)) => println!("TEXT {:?}", t),
                            Ok(credit_gateway::upstream::StreamEvent::Reasoning(t)) => println!("REASON {:?}", t.chars().take(40).collect::<String>()),
                            Ok(credit_gateway::upstream::StreamEvent::Done { finish }) => println!("DONE finish={finish}"),
                            Ok(credit_gateway::upstream::StreamEvent::Error(m, c)) => println!("ERROR {c}: {m}"),
                            Ok(_) => {}
                            Err(e) => println!("STREAM ERR {e}"),
                        }
                    }
                }
                Err(e) => println!("minimax tools ERR: {e}"),
            }
        }
        "serve" => {
            let port: u16 = std::env::args().nth(2).and_then(|p| p.parse().ok()).unwrap_or(8799);
            let store = std::sync::Arc::new(Store::open_default()?);
            let state = std::sync::Arc::new(credit_gateway::GatewayState {
                store: store.clone(),
                router: credit_gateway::Router::new(store),
                api_key: String::new(),
                rate_window: std::sync::Mutex::new(std::collections::VecDeque::new()),
                port,
                device_salt: std::sync::Mutex::new(credit_gateway::server::device_salt_init()),
                device_token: std::sync::Mutex::new(None),
                models_cache: std::sync::Mutex::new(None),
                models_info: std::sync::Mutex::new(std::collections::HashMap::new()),
            });
            let addr: std::net::SocketAddr = ([127, 0, 0, 1], port).into();
            println!("probe gateway on http://{addr}");
            credit_gateway::serve(state, addr).await?;
        }
        "qoder-models-live" => {
            let c = read_cred("qoder").ok_or_else(|| anyhow::anyhow!("no qoder cred"))?;
            let client = reqwest::Client::builder().build()?;
            let url = "https://gateway.qoder.com.cn/algo/api/v2/model/list";
            let resp = client
                .get(url)
                .header("Authorization", format!("Bearer {}", c.token))
                .header("Cosy-ClientType", "10")
                .header("Accept", "application/json")
                .header("User-Agent", "Qoder/0.5.14")
                .send()
                .await?;
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            println!("HTTP {status}\n{}", text.chars().take(3000).collect::<String>());
        }
        "qoder-raw" => {
            let c = read_cred("qoder").ok_or_else(|| anyhow::anyhow!("no qoder cred"))?;
            let model = std::env::args().nth(2).unwrap_or_else(|| "performance".into());
            let body = credit_gateway::protocol::qoder_chat_body(&[json!({"role":"user","content":"只回复 ok"})], &model);
            let signed = credit_gateway::upstream::qoder_sign_request(&c, body).await.map_err(|e| anyhow::anyhow!(e))?;
            let url = signed.get("url").and_then(|u| u.as_str()).unwrap_or("").to_string();
            let body_str = signed.get("body").and_then(|b| b.as_str()).unwrap_or("").to_string();
            let headers = signed.get("headers").cloned().unwrap_or(json!({}));
            println!("url={url}\nbody={body_str}\nheaders={}", serde_json::to_string(&headers).unwrap_or_default());
            let client = reqwest::Client::builder().build()?;
            let mut req = client.post(&url);
            if let Some(hs) = headers.as_object() {
                for (k, v) in hs { if let Some(sv) = v.as_str() { req = req.header(k, sv); } }
            }
            let resp = req.body(body_str).send().await?;
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            println!("HTTP {status}\nRAW:\n{}", text.chars().take(4000).collect::<String>());
        }
        "qoder-chat" => {
            let c = read_cred("qoder").ok_or_else(|| anyhow::anyhow!("no qoder cred"))?;
            let model = std::env::args().nth(2).unwrap_or_else(|| "mmodel".into());
            let body = credit_gateway::protocol::qoder_chat_body(&[json!({"role":"user","content":"只回复 ok"})], &model);
            match credit_gateway::upstream::qoder_chat_stream(&c, body).await {
                Ok(events) => {
                    use futures_util::StreamExt;
                    let mut ev = events;
                    let mut text = String::new();
                    let mut err = None;
                    while let Some(item) = ev.next().await {
                        match item {
                            Ok(credit_gateway::upstream::StreamEvent::Text(t)) => text.push_str(&t),
                            Ok(credit_gateway::upstream::StreamEvent::Error(m, _)) => err = Some(m),
                            Err(e) => err = Some(e),
                            _ => {}
                        }
                    }
                    println!("qoder[{model}] text={:?} err={:?}", text, err);
                }
                Err(e) => println!("qoder[{model}] chat ERR: {e}"),
            }
        }
        "qoder-all" => {
            let c = read_cred("qoder").ok_or_else(|| anyhow::anyhow!("no qoder cred"))?;
            let ids = ["auto", "fast", "efficient", "lite", "performance", "ultimate", "cmodel", "dfmodel", "dmodel", "gfmodel", "gm51model", "gmodel", "kmodel", "kmodel_latest", "mmodel", "q37fmodel", "qfmodel", "qmodel", "qmodel_38max", "qmodel_latest"];
            for model in ids {
                let body = credit_gateway::protocol::qoder_chat_body(&[json!({"role":"user","content":"只回复 ok"})], model);
                match credit_gateway::upstream::qoder_chat_stream(&c, body).await {
                    Ok(events) => {
                        use futures_util::StreamExt;
                        let mut ev = events;
                        let mut text = String::new();
                        let mut err = None;
                        let mut done = false;
                        while let Some(item) = ev.next().await {
                            match item {
                                Ok(credit_gateway::upstream::StreamEvent::Text(t)) => text.push_str(&t),
                                Ok(credit_gateway::upstream::StreamEvent::Done { .. }) => done = true,
                                Ok(credit_gateway::upstream::StreamEvent::Error(m, _)) => err = Some(m),
                                Err(e) => err = Some(e),
                                _ => {}
                            }
                        }
                        let head: String = text.chars().take(24).collect();
                        println!("qoder[{model}] ok done={done} text={head:?} err={err:?}");
                    }
                    Err(e) => println!("qoder[{model}] ERR: {e}"),
                }
            }
        }
        "zc-probe" => {
            // ZCode 备用通道实验：用本机凭据分别打 3 个候选 anthropic 端点
            let live = credit_core::adapters::zcode_local::read_live_auth()
                .ok_or_else(|| anyhow::anyhow!("no zcode local auth"))?;
            let jwt = live.zcode_jwt.clone().unwrap_or_default();
            let at = live.access_token.clone();
            println!("provider-token fp: jwt_len={} at_len={}", jwt.len(), at.len());
            let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build()?;
            let body = json!({
                "model": "GLM-5.3-Flash",
                "max_tokens": 64,
                "stream": false,
                "messages": [{ "role": "user", "content": "只回复 ok" }]
            });
            // 候选端点 × 候选 token 组合
            let endpoints = [
                ("plan-anthropic", "https://zcode.z.ai/api/v1/zcode-plan/anthropic/v1/messages"),
                ("open-bigmodel-anthropic", "https://open.bigmodel.cn/api/anthropic/v1/messages"),
                ("zai-anthropic", "https://api.z.ai/api/anthropic/v1/messages"),
            ];
            let tokens: Vec<(&str, &str)> = vec![
                ("zcodeJwt", &jwt),
                ("accessToken", &at),
            ];
            for (name, url) in endpoints {
                for (tname, tok) in &tokens {
                    if tok.is_empty() { continue; }
                    let resp = client.post(url)
                        .header("Authorization", format!("Bearer {tok}"))
                        .header("anthropic-version", "2023-06-01")
                        .header("Content-Type", "application/json")
                        .header("User-Agent", "ZCode/3.18.2")
                        .header("X-ZCode-App-Version", "3.18.2")
                        .header("X-Platform", "win32-x64")
                        .header("HTTP-Referer", "https://zcode.z.ai")
                        .header("X-Os-Category", "windows")
                        .header("X-Device-Mid", &live.device_mid)
                        .json(&body)
                        .send()
                        .await;
                    match resp {
                        Ok(r) => {
                            let st = r.status().as_u16();
                            let t = r.text().await.unwrap_or_default();
                            println!("[{name} + {tname}] HTTP {st}: {}", t.chars().take(220).collect::<String>());
                        }
                        Err(e) => println!("[{name} + {tname}] ERR {e}"),
                    }
                }
            }
        }
        "zc-endpoints" => {
            use credit_core::adapters::zcode_local;
            let Some(live) = zcode_local::read_live_auth() else { anyhow::bail!("no zcode live auth") };
            println!("provider_token_fp={} jwt_fp={} mid={}",
                live.access_token.chars().take(8).collect::<String>(),
                live.zcode_jwt.as_deref().map(|s| s.chars().take(10).collect::<String>()).unwrap_or_else(|| "-".into()),
                live.device_mid);
            let linked = zcode_local::read_credential("oauth:linked_bigmodel");
            println!("linked_bigmodel = {}", linked.as_deref().map(|s| format!("len={} head={}", s.len(), s.chars().take(60).collect::<String>())).unwrap_or_else(|| "none".into()));
            let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build()?;
            let body = json!({
                "model": "GLM-5.3-Flash",
                "max_tokens": 16,
                "stream": false,
                "messages": [{ "role": "user", "content": "只回复ok" }]
            });
            let full_headers = |tok: &str| vec![
                ("Authorization", format!("Bearer {tok}")),
                ("Content-Type", "application/json".into()),
                ("Accept", "application/json".into()),
                ("anthropic-version", "2023-06-01".into()),
                ("User-Agent", "ZCode/3.18.2".into()),
                ("HTTP-Referer", "https://zcode.z.ai".into()),
                ("X-Title", "Z Code@electron".into()),
                ("X-ZCode-App-Version", "3.18.2".into()),
                ("X-Platform", "win32-x64".into()),
                ("X-Client-Language", "zh-CN".into()),
                ("X-Client-Timezone", "Asia/Shanghai".into()),
                ("X-Os-Category", "windows".into()),
                ("X-Device-Mid", live.device_mid.clone()),
            ];
            let jwt = live.zcode_jwt.clone().unwrap_or_else(|| live.access_token.clone());
            let cases: Vec<(&str, &str, String)> = vec![
                ("start-plan(jwt)", "https://zcode.z.ai/api/v1/zcode-plan/anthropic/v1/messages", jwt.clone()),
                ("bigmodel-anthropic(jwt)", "https://open.bigmodel.cn/api/anthropic/v1/messages", jwt.clone()),
                ("bigmodel-anthropic(oauth)", "https://open.bigmodel.cn/api/anthropic/v1/messages", live.access_token.clone()),
                ("zai-anthropic(oauth)", "https://api.z.ai/api/anthropic/v1/messages", live.access_token.clone()),
                ("linked_bigmodel", "https://open.bigmodel.cn/api/anthropic/v1/messages", linked.clone().unwrap_or_default()),
            ];
            for (name, url, tok) in cases {
                if tok.is_empty() { println!("[{name}] skip (no token)"); continue; }
                let mut req = client.post(url);
                for (k, v) in full_headers(&tok) { req = req.header(k, v); }
                let resp = req.json(&body).send().await?;
                let st = resp.status().as_u16();
                let text = resp.text().await.unwrap_or_default();
                println!("[{name}] HTTP {st}: {}", text.chars().take(220).collect::<String>());
            }
        }
        "zc-verify" => {
            use credit_core::adapters::zcode::ZcodeAdapter;
            let c = read_cred("zcode").ok_or_else(|| anyhow::anyhow!("no zcode cred"))?;
            match credit_core::adapters::Adapter::verify(&ZcodeAdapter, &c).await {
                Ok(v) => println!("zcode verify OK: code={} (profile 取票成功)", v.get("verified").and_then(|x| x.as_bool()).unwrap_or(false)),
                Err(e) => println!("zcode verify ERR: {e}"),
            }
        }
        "zc-ticket" => {
            use credit_core::adapters::zcode::fresh_bigmodel_token;
            let c = read_cred("zcode").ok_or_else(|| anyhow::anyhow!("no zcode cred"))?;
            match fresh_bigmodel_token(&c).await {
                Ok(t) => println!("zcode profile ticket OK: len={} head={}", t.len(), t.chars().take(12).collect::<String>()),
                Err(e) => println!("zcode profile ticket ERR: {e}"),
            }
        }
        "zc-node" => {
            // 用 Node（不同 TLS 栈）复测 Start Plan 端点，排除 TLS 指纹门禁假设
            let live = credit_core::adapters::zcode_local::read_live_auth()
                .ok_or_else(|| anyhow::anyhow!("no zcode live auth"))?;
            let jwt = live.zcode_jwt.clone().unwrap_or_default();
            let mid = live.device_mid.clone();
            let script = r#"
const jwt = process.env.ZC_JWT, mid = process.env.ZC_MID;
const base = { 'User-Agent': 'ZCode/3.18.2', 'X-ZCode-App-Version': '3.18.2', 'X-Platform': 'win32-x64',
  'X-Client-Language': 'zh-CN', 'X-Client-Timezone': 'Asia/Shanghai', 'X-Os-Category': 'windows', 'X-Device-Mid': mid };
const p = await fetch('https://zcode.z.ai/api/v1/oauth/profile', { headers: { ...base, Authorization: `Bearer ${jwt}` } });
const t = await p.json();
const tok = t?.data?.bigmodel?.access_token || '';
console.log('profile', p.status, 'token_len=', tok.length);
async function tryChat(label, token, model) {
  if (!token) { console.log(label, 'skip no token'); return; }
  const r = await fetch('https://zcode.z.ai/api/v1/zcode-plan/anthropic/v1/messages', {
    method: 'POST',
    headers: { ...base, Authorization: `Bearer ${token}`, 'anthropic-version': '2023-06-01', 'Content-Type': 'application/json' },
    body: JSON.stringify({ model, max_tokens: 16, stream: false, messages: [{ role: 'user', content: 'reply ok' }] }),
  });
  console.log(label, model, r.status, (await r.text()).slice(0, 160));
}
// 同一模型连打 6 次，观察门禁是否随机（边缘节点差异）
for (let i = 0; i < 6; i++) {
  await tryChat(`try${i}`, jwt, 'GLM-5.3-Flash');
}
"#;
            let out = tokio::process::Command::new("node")
                .arg("--input-type=module")
                .arg("-e")
                .arg(script)
                .env("ZC_JWT", &jwt)
                .env("ZC_MID", &mid)
                .output()
                .await
                .map_err(|e| anyhow::anyhow!("node spawn: {e}"))?;
            println!("stdout: {}", String::from_utf8_lossy(&out.stdout));
            let err = String::from_utf8_lossy(&out.stderr);
            if !err.trim().is_empty() { println!("stderr: {}", err.chars().take(600).collect::<String>()); }
        }
        _ => println!("unknown cmd {cmd}"),
    }
    Ok(())
}
