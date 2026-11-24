//! 隔离实验 3：复现 DSH 真实请求形状（多轮 + 历史 tool 结果 + tool_choice），
//! 定位「不执行工具 / 退化成纯文本」的真实触发条件。
//!
//! 变量：
//!   W1 = 单轮 + tools + tool_choice=auto
//!   W2 = 单轮 + tools + 无 tool_choice
//!   W3 = 单轮 + tools + tool_choice=none          ← 关键嫌疑：DSH 可能发 "none"
//!   W4 = 单轮 + tools + tool_choice=required
//!   W5 = 单轮 + tools + tool_choice={"type":"function",...}
//!   W6 = 多轮（含历史 assistant.tool_calls + tool 结果）
//!   W7 = 多轮 + tool_choice=none
//!
//! 用法：cargo run --offline -p credit-gateway --example qisolate3 -- [model]

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

struct Stat { text: usize, tool: usize, names: Vec<String>, first: String, ms: u128, raw_len: usize }

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

    let mut st = Stat { text: 0, tool: 0, names: vec![], first: String::new(), ms: 0, raw_len: raw.len() };
    for v in &chunks {
        for ch in v.get("choices").and_then(|c| c.as_array()).unwrap_or(&vec![]) {
            let delta = ch.get("delta").cloned().unwrap_or(Value::Null);
            if let Some(t) = delta.get("content").and_then(|c| c.as_str()) {
                if !t.is_empty() { st.text += 1; if st.first.is_empty() { st.first = t.to_string(); } }
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
    println!("[{label}] {mode} text={} tool={} {}ms raw={}B tools={:?}",
        st.text, st.tool, st.ms, st.raw_len, st.names);
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
    let user_msg = format!("列出 `{cwd}` 目录下的文件，并读取 Cargo.toml 的前 20 行。");

    // 单轮构造器：tool_choice 可选
    let mk1 = |tc: Option<Value>| -> Value {
        let messages = vec![json!({ "role": "user", "content": user_msg.clone() })];
        let mut b = credit_gateway::protocol::qoder_chat_body(&messages, "", &model);
        credit_gateway::protocol::attach_openai_tools(&mut b, &tools, tc.as_ref());
        b
    };

    report("W1 tc=auto",     &probe(&cred, mk1(Some(json!("auto")))).await?);
    report("W2 无tc",        &probe(&cred, mk1(None)).await?);
    report("W3 tc=none",     &probe(&cred, mk1(Some(json!("none")))).await?);
    report("W4 tc=required", &probe(&cred, mk1(Some(json!("required")))).await?);
    report("W5 tc=function", &probe(&cred, mk1(Some(json!({
        "type": "function", "function": { "name": "list_dir" } })))).await?);

    // 多轮：含历史 assistant.tool_calls + tool 结果（DSH 第二轮的典型形状）
    let mk2 = |tc: Option<Value>| -> Value {
        let messages = vec![
            json!({ "role": "system", "content": format!("You are running in `{cwd}`.") }),
            json!({ "role": "user", "content": user_msg.clone() }),
            json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "call_1", "type": "function",
                    "function": { "name": "list_dir", "arguments": format!("{{\"path\":\"{}\"}}", cwd) }
                }]
            }),
            json!({ "role": "tool", "tool_call_id": "call_1", "content": "Cargo.toml  crates  src  docs" }),
            json!({ "role": "user", "content": "继续读取 Cargo.toml 的前 20 行。" }),
        ];
        let mut b = credit_gateway::protocol::qoder_chat_body(&messages, "", &model);
        credit_gateway::protocol::attach_openai_tools(&mut b, &tools, tc.as_ref());
        b
    };
    report("W6 多轮 tc=auto", &probe(&cred, mk2(Some(json!("auto")))).await?);
    report("W7 多轮 tc=none", &probe(&cred, mk2(Some(json!("none")))).await?);

    Ok(())
}
