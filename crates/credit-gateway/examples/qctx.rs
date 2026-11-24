//! 诊断探针：复现 DSH → 网关 → Qoder 的真实请求形状，定位
//! 「模型不执行分析」+「项目目录不对」的根因。
//!
//! 关键对照：
//!   A 组 = 现在的 `qoder_chat_body`（无 system、无 chat_context）
//!   B 组 = 官方客户端形状（system + chat_context(cwd) + tools）
//! 打印两组的完整 body diff，并直打上游看返回差异。
//!
//! 用法：cargo run -p credit-gateway --example qctx -- [model]
//!      默认 model=gfmodel

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

/// DSH 实际会发的 system 提示词（简化版，含 cwd 语义）
fn dsh_system(cwd: &str) -> String {
    format!(
        "You are running in the project directory `{cwd}`.\n\
         Always use absolute paths rooted at that directory for file operations.\n\
         When the user asks you to analyse a project, first list its files, then reason about them."
    )
}

/// DSH 的 `read_file` / `list_dir` 工具（简化）
fn dsh_tools() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a file from the project directory.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Absolute path inside the project directory." }
                    },
                    "required": ["path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "list_dir",
                "description": "List files in a directory inside the project directory.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Absolute path inside the project directory." }
                    },
                    "required": ["path"]
                }
            }
        }),
    ]
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let model = std::env::args().nth(1).unwrap_or_else(|| "gfmodel".into());
    let cwd = std::env::current_dir()?.display().to_string();

    let cred = read_cred("qoder").ok_or_else(|| anyhow::anyhow!("no qoder cred in DB"))?;
    println!("=== model={model}  cwd={cwd} ===\n");

    let tools = dsh_tools();
    let messages = vec![json!({
        "role": "user",
        "content": format!("列出 `{cwd}` 目录下的文件，并读取 Cargo.toml 的前 20 行。")
    })];

    // ── A 组：当前网关实现（system 被丢弃，无 chat_context）──
    let body_a = credit_gateway::protocol::qoder_chat_body(&messages, "", &model);
    credit_gateway::protocol::attach_openai_tools(&mut body_a.clone(), &tools, Some(&json!("auto")));

    // ── B 组：带 system + chat_context 的官方形状 ──
    let mut body_b = credit_gateway::protocol::qoder_chat_body(&messages, "", &model);
    credit_gateway::protocol::attach_openai_tools(&mut body_b, &tools, Some(&json!("auto")));
    body_b["system"] = json!(dsh_system(&cwd));
    body_b["chat_context"] = json!({
        "cwd": cwd,
        "workspace_root": cwd,
        "project_name": "credit-rs",
    });
    body_b["custom_context"] = json!({ "cwd": cwd });

    println!("=== A 组（当前实现）body 顶层键 ===");
    let ka: Vec<&String> = body_a.as_object().unwrap().keys().collect();
    println!("{:?}\n", ka);
    println!("=== B 组（官方形状）body 顶层键 ===");
    let kb: Vec<&String> = body_b.as_object().unwrap().keys().collect();
    println!("{:?}\n", kb);

    println!("=== A 组是否含 system？ === {}", body_a.get("system").is_some());
    println!("=== A 组是否含 chat_context？ === {}", body_a.get("chat_context").is_some());
    println!("=== A 组是否含 cwd 任何位置？ === {}\n",
        body_a.to_string().contains(&cwd));

    // ── 直打上游，对比两组的返回 ──
    for (label, body) in [("A(当前实现)", body_a), ("B(带system+ctx)", body_b)] {
        println!("──────── {label} 直打上游 ────────");
        let signed = credit_gateway::upstream::qoder_sign_request(&cred, body.clone())
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
        let status = resp.status().as_u16();
        let raw = resp.text().await.unwrap_or_default();
        println!("HTTP {status}  {}ms  {} bytes", t0.elapsed().as_millis(), raw.len());

        // 解析外层信封 → 内层 chunk，统计事件类型
        let mut n_text = 0usize;
        let mut n_reason = 0usize;
        let mut n_tool = 0usize;
        let mut n_err = 0usize;
        let mut texts: Vec<String> = Vec::new();
        for line in raw.lines() {
            let Some(d) = line.trim_start().strip_prefix("data:") else { continue };
            let d = d.trim();
            if d.is_empty() || d == "[DONE]" { continue; }
            let Ok(env) = serde_json::from_str::<Value>(d) else { continue };
            let inner = env.get("body").and_then(|b| b.as_str()).unwrap_or("");
            if inner.trim().starts_with('{') {
                if let Ok(v) = serde_json::from_str::<Value>(inner) {
                    for ch in v.get("choices").and_then(|c| c.as_array()).unwrap_or(&vec![]) {
                        let delta = ch.get("delta").cloned().unwrap_or(Value::Null);
                        if let Some(t) = delta.get("content").and_then(|c| c.as_str()) {
                            if !t.is_empty() { n_text += 1; texts.push(t.to_string()); }
                        }
                        if let Some(t) = delta.get("reasoning_content").and_then(|c| c.as_str()) {
                            if !t.is_empty() { n_reason += 1; }
                        }
                        if let Some(tcs) = delta.get("tool_calls").and_then(|c| c.as_array()) {
                            n_tool += tcs.len();
                        }
                    }
                    if v.get("code").is_some() && v.get("choices").is_none() {
                        n_err += 1;
                        println!("  业务错误: code={} msg={}", v.get("code").unwrap(), v.get("message").unwrap_or(&Value::Null));
                    }
                }
            } else if inner.trim().starts_with("data:") {
                // 内层 SSE 文本
                for l2 in inner.lines() {
                    let Some(d2) = l2.trim_start().strip_prefix("data:") else { continue };
                    let d2 = d2.trim();
                    if d2.is_empty() || d2 == "[DONE]" { continue; }
                    if let Ok(v) = serde_json::from_str::<Value>(d2) {
                        for ch in v.get("choices").and_then(|c| c.as_array()).unwrap_or(&vec![]) {
                            let delta = ch.get("delta").cloned().unwrap_or(Value::Null);
                            if let Some(t) = delta.get("content").and_then(|c| c.as_str()) {
                                if !t.is_empty() { n_text += 1; texts.push(t.to_string()); }
                            }
                            if let Some(t) = delta.get("reasoning_content").and_then(|c| c.as_str()) {
                                if !t.is_empty() { n_reason += 1; }
                            }
                            if let Some(tcs) = delta.get("tool_calls").and_then(|c| c.as_array()) {
                                n_tool += tcs.len();
                            }
                        }
                    }
                }
            }
        }
        println!("  文本分片={n_text}  思考分片={n_reason}  工具调用={n_tool}  错误={n_err}");
        let joined: String = texts.concat();
        println!("  文本前 300 字: {}", joined.chars().take(300).collect::<String>());
        println!("  文本中含 cwd 路径? {}", joined.contains(&cwd));
        println!();
    }
    Ok(())
}
