//! 隔离实验 2：定位「Qoder 不执行工具、退化成纯文本」的真实触发条件。
//!
//! 假设：模型在**首轮**就自行决定「我无法访问文件系统」并直接作纯文本回答，
//! 于是工具调用分片为 0；一旦首轮进入 agent 模式就会下发 tool_calls。
//! 需要找出：什么让首轮退化为纯文本？
//!
//! 变量：
//!   V1 = 单步请求（只要求 list_dir）
//!   V2 = 多步请求（list_dir + read_file）
//!   V3 = 多步请求，但把 cwd 放进 system
//!   V4 = 多步请求 + chat_context
//!   V5 = 多步请求 + system + chat_context
//!
//! 用法：cargo run --offline -p credit-gateway --example qisolate2 -- [model]

use credit_core::adapters::Cred;
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

fn dsh_tools() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "list_dir",
                "description": "List files in a directory inside the project directory.",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a file from the project directory.",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"]
                }
            }
        }),
    ]
}

struct Stat { text: usize, reason: usize, tool: usize, names: Vec<String>, first: String, ms: u128 }

async fn probe(cred: &Cred, body: Value) -> anyhow::Result<Stat> {
    let signed = credit_gateway::upstream::qoder_sign_request(cred, body.clone())
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    let url = signed.get("url").and_then(|u| u.as_str()).unwrap_or("").to_string();
    let body_str = signed.get("body").and_then(|b| b.as_str()).unwrap_or("").to_string();
    let headers = signed.get("headers").cloned().unwrap_or(json!({}));

    let client = reqwest::Client::builder().build()?;
    let mut req = client.post(&url);
    if let Some(hs) = headers.as_object() {
        for (k, v) in hs {
            if let Some(sv) = v.as_str() { req = req.header(k, sv); }
            }
        }
    let t0 = std::time::Instant::now();
    let resp = req.body(body_str).send().await?;
    let raw = resp.text().await.unwrap_or_default();

    let mut chunks: Vec<Value> = Vec::new();
    for line in raw.lines() {
        let Some(d) = line.trim_start().strip_prefix("data:") else { continue };
        let d = d.trim();
        if d.is_empty() || d == "[DONE]" { continue; }
        let Ok(env) = serde_json::from_str::<Value>(d) else { continue };
        let inner = env.get("body").and_then(|b| b.as_str()).unwrap_or("");
        if inner.trim().starts_with('{') {
            if let Ok(v) = serde_json::from_str::<Value>(inner) { chunks.push(v); }
        } else {
            for l2 in inner.lines() {
                let Some(d2) = l2.trim_start().strip_prefix("data:") else { continue };
                let d2 = d2.trim();
                if d2.is_empty() || d2 == "[DONE]" { continue; }
                if let Ok(v) = serde_json::from_str::<Value>(d2) { chunks.push(v); }
            }
        }
    }

    let mut st = Stat { text: 0, reason: 0, tool: 0, names: vec![], first: String::new(), ms: 0 };
    for v in &chunks {
        for ch in v.get("choices").and_then(|c| c.as_array()).unwrap_or(&vec![]) {
            let delta = ch.get("delta").cloned().unwrap_or(Value::Null);
            if let Some(t) = delta.get("content").and_then(|c| c.as_str()) {
                if !t.is_empty() { st.text += 1; if st.first.is_empty() { st.first = t.to_string(); } }
            }
            if let Some(t) = delta.get("reasoning_content").and_then(|c| c.as_str()) {
                if !t.is_empty() { st.reason += 1; }
            }
            if let Some(tcs) = delta.get("tool_calls").and_then(|c| c.as_array()) {
                for tc in tcs {
                    st.tool += 1;
                    let f = tc.get("function").cloned().unwrap_or(Value::Null);
                    if let Some(n) = f.get("name").and_then(|x| x.as_str()) {
                        if !st.names.contains(&n.to_string()) { st.names.push(n.to_string()); }
                    }
                }
            }
        }
    }
    st.ms = t0.elapsed().as_millis();
    Ok(st)
}

fn report(label: &str, st: &Stat) {
    let mode = if st.tool > 0 { "AGENT ✔" } else { "CHAT ✘" };
    println!("[{label}] {mode} text={} reason={} tool={} {}ms  tools={:?}",
        st.text, st.reason, st.tool, st.ms, st.names);
    if !st.first.is_empty() {
        println!("      首段: {}", st.first.chars().take(110).collect::<String>());
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let model = std::env::args().nth(1).unwrap_or_else(|| "gfmodel".into());
    let cwd = std::env::current_dir()?.display().to_string();
    let cred = read_cred("qoder").ok_or_else(|| anyhow::anyhow!("no qoder cred in DB"))?;
    println!("=== model={model} cwd={cwd} ===\n");

    let tools = dsh_tools();

    let mk = |msg: &str| -> Value {
        let messages = vec![json!({ "role": "user", "content": msg })];
        let mut b = credit_gateway::protocol::qoder_chat_body(&messages, "", &model);
        credit_gateway::protocol::attach_openai_tools(&mut b, &tools, Some(&json!("auto")));
        b
    };

    // V1 单步
    report("V1 单步(list)", &probe(&cred, mk(&format!("列出 `{cwd}` 目录下的文件。"))).await?);

    // V2 多步
    report("V2 多步(list+read)", &probe(&cred, mk(&format!("列出 `{cwd}` 目录下的文件，并读取 Cargo.toml 的前 20 行。"))).await?);

    // V3 多步 + system
    let mut b = mk(&format!("列出 `{cwd}` 目录下的文件，并读取 Cargo.toml 的前 20 行。"));
    b["system"] = json!(format!("You are running in the project directory `{cwd}`. Use tools to inspect it."));
    report("V3 多步+system", &probe(&cred, b).await?);

    // V4 多步 + chat_context
    let mut b = mk(&format!("列出 `{cwd}` 目录下的文件，并读取 Cargo.toml 的前 20 行。"));
    b["chat_context"] = json!({ "cwd": cwd, "workspace_root": cwd });
    report("V4 多步+ctx", &probe(&cred, b).await?);

    // V5 多步 + system + chat_context
    let mut b = mk(&format!("列出 `{cwd}` 目录下的文件，并读取 Cargo.toml 的前 20 行。"));
    b["system"] = json!(format!("You are running in the project directory `{cwd}`. Use tools to inspect it."));
    b["chat_context"] = json!({ "cwd": cwd, "workspace_root": cwd });
    report("V5 多步+system+ctx", &probe(&cred, b).await?);

    // V6 多步 + system 里明确要求用工具
    let mut b = mk(&format!("列出 `{cwd}` 目录下的文件，并读取 Cargo.toml 的前 20 行。"));
    b["system"] = json!(format!(
        "You are an agent running in the project directory `{cwd}`.\n\
         You MUST use the provided tools to inspect the filesystem. Do not answer from memory."
    ));
    report("V6 多步+system(强制用工具)", &probe(&cred, b).await?);

    Ok(())
}
