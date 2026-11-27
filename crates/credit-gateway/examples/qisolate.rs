//! 隔离实验：判定 Qoder 上游进入「agent 模式」到底靠哪个字段。
//!
//! 逐组递增字段，观察「工具调用数」与「文本分片数」的变化：
//!   G0 = 基准（当前实现，无 system/chat_context）
//!   G1 = G0 + system
//!   G2 = G0 + chat_context{cwd,...}
//!   G3 = G0 + system + chat_context
//!   G4 = G0 + system + chat_context + custom_context
//! 判定标准：工具调用 > 0 ⇒ 进入 agent 模式（会真正执行任务，而不是纯文本作答）。
//!
//! 用法：cargo run --offline -p credit-gateway --example qisolate -- [model]

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

/// 统计一组 body 直打上游后返回的：文本分片 / 思考分片 / 工具调用 / 错误
async fn probe(label: &str, cred: &Cred, _model: &str, body: Value) -> anyhow::Result<()> {
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
    let _status = resp.status().as_u16();
    let raw = resp.text().await.unwrap_or_default();

    let (mut n_text, mut n_reason, mut n_tool, mut n_err) = (0usize, 0usize, 0usize, 0usize);
    let mut tool_names: Vec<String> = Vec::new();
    let mut first_text = String::new();
    let mut err_msg = String::new();

    // 外层信封的 body 可能是裸 chunk JSON，也可能是内层 SSE 文本
    let mut inner_chunks: Vec<Value> = Vec::new();
    for line in raw.lines() {
        let Some(d) = line.trim_start().strip_prefix("data:") else { continue };
        let d = d.trim();
        if d.is_empty() || d == "[DONE]" { continue; }
        let Ok(env) = serde_json::from_str::<Value>(d) else { continue };
        let inner = env.get("body").and_then(|b| b.as_str()).unwrap_or("");
        if inner.trim().starts_with('{') {
            if let Ok(v) = serde_json::from_str::<Value>(inner) { inner_chunks.push(v); }
        } else {
            for l2 in inner.lines() {
                let Some(d2) = l2.trim_start().strip_prefix("data:") else { continue };
                let d2 = d2.trim();
                if d2.is_empty() || d2 == "[DONE]" { continue; }
                if let Ok(v) = serde_json::from_str::<Value>(d2) { inner_chunks.push(v); }
            }
        }
    }
    for v in &inner_chunks {
        if v.get("code").is_some() && v.get("choices").is_none() {
            n_err += 1;
            err_msg = format!("code={} msg={}", v.get("code").unwrap(), v.get("message").unwrap_or(&Value::Null));
        }
        for ch in v.get("choices").and_then(|c| c.as_array()).unwrap_or(&vec![]) {
            let delta = ch.get("delta").cloned().unwrap_or(Value::Null);
            if let Some(t) = delta.get("content").and_then(|c| c.as_str()) {
                if !t.is_empty() { n_text += 1; if first_text.is_empty() { first_text = t.to_string(); } }
            }
            if let Some(t) = delta.get("reasoning_content").and_then(|c| c.as_str()) {
                if !t.is_empty() { n_reason += 1; }
            }
            if let Some(tcs) = delta.get("tool_calls").and_then(|c| c.as_array()) {
                for tc in tcs {
                    n_tool += 1;
                    let f = tc.get("function").cloned().unwrap_or(Value::Null);
                    if let Some(n) = f.get("name").and_then(|x| x.as_str()) {
                        if !tool_names.contains(&n.to_string()) { tool_names.push(n.to_string()); }
                    }
                }
            }
        }
    }

    let mode = if n_tool > 0 { "AGENT ✔" } else { "CHAT ✘" };
    println!("[{label}] {mode}  text={n_text} reason={n_reason} tool={n_tool} err={n_err}  {}ms",
        t0.elapsed().as_millis());
    if !tool_names.is_empty() { println!("        tools: {:?}", tool_names); }
    if !err_msg.is_empty() { println!("        err: {err_msg}"); }
    if !first_text.is_empty() {
        println!("        首段文本: {}", first_text.chars().take(120).collect::<String>());
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let model = std::env::args().nth(1).unwrap_or_else(|| "gfmodel".into());
    let cwd = std::env::current_dir()?.display().to_string();

    let cred = read_cred("qoder").ok_or_else(|| anyhow::anyhow!("no qoder cred in DB"))?;
    println!("=== model={model} cwd={cwd} ===\n");

    let tools = dsh_tools();
    let messages = vec![json!({
        "role": "user",
        "content": format!("列出 `{cwd}` 目录下的文件。")
    })];

    let base = || {
        let mut b = credit_gateway::protocol::qoder_chat_body(&messages, "", &model);
        credit_gateway::protocol::attach_openai_tools(&mut b, &tools, Some(&json!("auto")));
        b
    };

    // G0 基准
    probe("G0 基准(无system/无ctx)", &cred, &model, base()).await?;

    // G1 = + system
    let mut b = base();
    b["system"] = json!(format!("You are running in the project directory `{cwd}`."));
    probe("G1 +system", &cred, &model, b).await?;

    // G2 = + chat_context
    let mut b = base();
    b["chat_context"] = json!({ "cwd": cwd, "workspace_root": cwd });
    probe("G2 +chat_context", &cred, &model, b).await?;

    // G3 = + system + chat_context
    let mut b = base();
    b["system"] = json!(format!("You are running in the project directory `{cwd}`."));
    b["chat_context"] = json!({ "cwd": cwd, "workspace_root": cwd });
    probe("G3 +system+chat_context", &cred, &model, b).await?;

    // G4 = + system + chat_context + custom_context
    let mut b = base();
    b["system"] = json!(format!("You are running in the project directory `{cwd}`."));
    b["chat_context"] = json!({ "cwd": cwd, "workspace_root": cwd });
    b["custom_context"] = json!({ "cwd": cwd });
    probe("G4 +system+ctx+custom", &cred, &model, b).await?;

    Ok(())
}
