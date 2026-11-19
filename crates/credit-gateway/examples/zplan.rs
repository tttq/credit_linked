//! ZCode Start Plan 端到端实测（走网关真实代码路径 `upstream::zcode_chat_stream`）。
//!
//! 用法：`cargo run -p credit-gateway --example zplan -- [prompt]`
//!
//! 验证 2026-11-19 定位的 3012 闸门修复是否生效：
//!   ① `X-ZCode-App-Version: 3.18.2`
//!   ② `system` 前两块 = 官方 CLI 固定文本（CLI Prefix + Identity/Harness）
use serde_json::json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let prompt = std::env::args().nth(1).unwrap_or_else(|| "用一句话介绍你自己".into());

    let live = credit_core::adapters::zcode_local::read_live_auth()
        .ok_or_else(|| anyhow::anyhow!("no live zcode auth（请先登录 ZCode 客户端）"))?;
    let jwt = live.zcode_jwt.clone().unwrap_or_default();
    let mid = live.device_mid.clone();

    let mut cred = credit_core::adapters::Cred::default();
    cred.provider = "zcode".into();
    cred.meta = serde_json::json!({ "zcodeJwt": jwt, "deviceMid": mid });

    // 走 OpenAI 兼容入参（网关对外形态），网关内部转 Anthropic 线格式
    let body = json!({
        "model": "GLM-5.3-Flash",
        "max_tokens": 256,
        "messages": [{ "role": "user", "content": prompt }],
        "stream": true,
        // 工具调用自检：让模型调用 get_weather 工具
        "tools": [{
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "查询天气",
                "parameters": {
                    "type": "object",
                    "properties": { "city": { "type": "string" } },
                    "required": ["city"],
                },
            },
        }],
        "tool_choice": "auto",
    });

    println!("→ 调用 credit_gateway::upstream::zcode_chat_stream …");
    match credit_gateway::upstream::zcode_chat_stream(&cred, body).await {
        Ok(stream) => {
            use futures_util::StreamExt;
            let mut stream = stream;
            let mut out = String::new();
            let mut events = 0usize;
            while let Some(item) = stream.next().await {
                match item {
                    Ok(chunk) => {
                        events += 1;
                        out.push_str(&format!("{chunk:?}"));
                        if events > 200 { break; }
                    }
                    Err(e) => { eprintln!("stream error: {e}"); break; }
                }
            }
            println!("✓ 收到 {events} 个 chunk");
            println!("--- 响应 ---\n{out}");
        }
        Err(e) => {
            eprintln!("✗ 失败: {e}");
            std::process::exit(1);
        }
    }
    Ok(())
}
