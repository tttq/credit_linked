//! 逆向探针：把 DSH 侧 `skill` 工具 schema 原样发给 Qoder 上游，
//! dump **未经任何清洗** 的原始 SSE 帧，用来判定「工具参数丢失」发生在哪一层：
//!   上游没产参数 / 上游产了异形字段 / 网关解析丢了 / 网关 sanitize 归零。
//!
//! 用法：cargo run --offline -p credit-gateway --example qdump -- [model] [outfile]
//!      默认 model=gfmodel，outfile=%TEMP%/qdump-raw.txt

use credit_core::adapters::Cred;
use credit_core::db::Store;
use credit_gateway::protocol::{attach_openai_tools, qoder_chat_body};
use credit_gateway::upstream::qoder_sign_request;
use serde_json::{json, Value};

fn read_cred(provider: &str) -> Option<Cred> {
    let store = Store::open_default().ok()?;
    let rows = store.list_accounts().ok()?;
    rows.iter()
        .filter(|(p, _, _, _, _)| p == provider)
        .filter_map(|(_, _, _, _, auth)| serde_json::from_str::<Cred>(auth).ok())
        .next()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let model = std::env::args().nth(1).unwrap_or_else(|| "gfmodel".into());
    let out_path = std::env::args().nth(2).unwrap_or_else(|| {
        format!("{}/qdump-raw-{}.txt", std::env::temp_dir().display(), model)
    });

    let cred = read_cred("qoder").ok_or_else(|| anyhow::anyhow!("no qoder cred in DB"))?;

    // 与 DSH `skill` 工具同形（required: ["name"]）
    let tools = vec![json!({
        "type": "function",
        "function": {
            "name": "skill",
            "description": "Load the full instructions for a skill. Call it before acting on a task that names or clearly matches a skill in the session skill catalog.",
            "parameters": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The exact skill name from the available skills list."
                    }
                },
                "required": ["name"]
            }
        }
    })];
    let messages = vec![json!({
        "role": "user",
        "content": "可用技能目录里只有 novel-comic-drama 一个技能。请立刻调用 skill 工具加载它（name 参数填技能名），不要输出其它文字。"
    })];

    let mut body = qoder_chat_body(&messages, &model);
    attach_openai_tools(&mut body, &tools, Some(&json!("auto")));
    println!("=== 送上游的 body（tools 部分）===");
    println!("{}", serde_json::to_string_pretty(body.get("tools").unwrap_or(&Value::Null))?);

    let signed = qoder_sign_request(&cred, body.clone()).await.map_err(|e| anyhow::anyhow!(e))?;
    let url = signed.get("url").and_then(|u| u.as_str()).unwrap_or("").to_string();
    let body_str = signed.get("body").and_then(|b| b.as_str()).unwrap_or("").to_string();
    let headers = signed.get("headers").cloned().unwrap_or(json!({}));
    println!("\n=== signed url ===\n{url}");
    println!("\n=== signed body（实际发出的）===\n{body_str}");

    let client = reqwest::Client::builder().build()?;
    let mut req = client.post(&url);
    if let Some(hs) = headers.as_object() {
        for (k, v) in hs {
            if let Some(sv) = v.as_str() {
                req = req.header(k, sv);
            }
        }
    }
    let t0 = std::time::Instant::now();
    let resp = req.body(body_str).send().await?;
    let status = resp.status().as_u16();
    let raw = resp.text().await.unwrap_or_default();
    println!("\n=== HTTP {status}，{}ms，raw {} bytes ===", t0.elapsed().as_millis(), raw.len());

    // 原样落盘：一行一帧，便于逐帧看 tool_calls 参数分片
    let mut out = String::new();
    for line in raw.lines() {
        out.push_str(line);
        out.push('\n');
    }
    std::fs::write(&out_path, &out)?;
    println!("raw dump -> {out_path}");
    println!("\n=== 前 6000 字节 ===\n{}", raw.chars().take(6000).collect::<String>());
    Ok(())
}
