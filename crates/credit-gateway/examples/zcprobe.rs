//! ZCode 渠道校验实测探针：对两条通道逐个探测，打印精确的 HTTP 状态与响应体。
//!
//! 用法：cargo run -p credit-gateway --example zcprobe -- [model]
//!
//! 目标：搞清 11128（unapproved channel）/ 3012（非官方客户端）/ 1113（无权益）
//! 各自在哪种请求指纹下触发，从而确定哪条通道仍可用。
use serde_json::{json, Value};

fn live_jwt_mid() -> Option<(String, String)> {
    let a = credit_core::adapters::zcode_local::read_live_auth()?;
    Some((a.zcode_jwt.unwrap_or_default(), a.device_mid))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let model = std::env::args().nth(1).unwrap_or_else(|| "GLM-5.3-Flash".into());
    let (jwt, mid) = live_jwt_mid().ok_or_else(|| anyhow::anyhow!("no live zcode auth"))?;
    if jwt.is_empty() {
        anyhow::bail!("本机无 zcodejwttoken");
    }
    println!("jwt_len={} mid={mid} model={model}", jwt.len());

    // 官方 CLI 固定 system 块（现有实现）
    let sys = credit_gateway::protocol::zcode_start_plan_system_blocks("");
    let body = json!({
        "model": model,
        "max_tokens": 64,
        "stream": true,
        "system": sys,
        "messages": [{ "role": "user", "content": [{ "type": "text", "text": "reply ok" }] }],
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .build()?;

    // 通道 A：Start Plan 专属端点（官方同款）
    probe(&client, "A StartPlan",
        "https://zcode.z.ai/api/v1/zcode-plan/anthropic/v1/messages",
        &jwt, &mid, &body).await;

    // 通道 B：bigmodel Coding-Plan Anthropic 端点（需 profile 取票）
    let cred = credit_core::adapters::Cred {
        provider: "zcode".into(),
        meta: json!({ "zcodeJwt": jwt, "deviceMid": mid }),
        ..Default::default()
    };
    let fresh = match credit_core::adapters::zcode::fresh_bigmodel_token(&cred).await {
        Ok(t) => Some(t),
        Err(e) => { println!("[B bigmodel] 取票失败: {e}"); None }
    };
    if let Some(tok) = &fresh {
        probe(&client, "B bigmodel+官方完整头", "https://open.bigmodel.cn/api/anthropic/v1/messages",
            tok, &mid, &body).await;
        // 通道 D：同一个 bigmodel 端点，但用**网关当前回退路径的头集**（zcode_chat_headers）
        probe_with_chat_headers(&client, "D bigmodel+网关旧头集",
            "https://open.bigmodel.cn/api/anthropic/v1/messages", tok, &mid, &body).await;
    }

    // 通道 C：Start Plan 端点但不带官方 system 块（看是否只差这个）
    let mut b2 = body.clone();
    b2["system"] = json!([{ "type": "text", "text": "You are a helpful assistant" }]);
    probe(&client, "C StartPlan-system变体",
        "https://zcode.z.ai/api/v1/zcode-plan/anthropic/v1/messages",
        &jwt, &mid, &b2).await;

    // 通道 E：智谱开放平台 API Key 通道（用假 key，验证端点/请求形态/错误映射）
    let e_body = json!({
        "model": model,
        "max_tokens": 32,
        "messages": [{ "role": "user", "content": "reply ok" }],
        "system": "You are a helpful assistant",
        "reasoning_effort": "low",
    });
    match credit_gateway::upstream::zcode_apikey_chat_stream("invalid-test-key-000", e_body).await {
        Ok(_) => println!("[E apikey] 意外成功（不应发生）"),
        Err(e) => println!("[E apikey] 错误映射结果：\n    {e}"),
    }

    // 通道 F：走网关完整 zcode_chat_stream（无 apiKey），看最终提示是否给出稳定解法
    let fcred = credit_core::adapters::Cred {
        provider: "zcode".into(),
        meta: json!({ "zcodeJwt": jwt, "deviceMid": mid }),
        ..Default::default()
    };
    let f_body = json!({
        "model": model,
        "messages": [{ "role": "user", "content": "reply ok" }],
        "system": "",
    });
    match credit_gateway::upstream::zcode_chat_stream(&fcred, f_body).await {
        Ok(_) => println!("[F zcode_chat_stream] 成功"),
        Err(e) => println!("[F zcode_chat_stream] 最终错误：\n    {e}"),
    }

    Ok(())
}

/// 用网关旧的 `zcode_chat_headers`（精简头集）发请求，验证 11128 是否由此触发
async fn probe_with_chat_headers(client: &reqwest::Client, tag: &str, url: &str, token: &str, mid: &str, body: &Value) {
    let mut hs = credit_gateway::protocol::zcode_chat_headers(token, Some(mid));
    hs.push(("anthropic-version".to_string(), "2023-06-01".to_string()));
    let mut req = client.post(url).json(body);
    for (k, v) in &hs {
        req = req.header(k, v);
    }
    match req.send().await {
        Ok(resp) => {
            let st = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            println!("[{tag}] HTTP {st}\n    body={}", text.chars().take(320).collect::<String>());
        }
        Err(e) => println!("[{tag}] 请求失败: {e}"),
    }
}

async fn probe(client: &reqwest::Client, tag: &str, url: &str, token: &str, mid: &str, body: &Value) {
    let hs = credit_gateway::protocol::zcode_plan_chat_headers(token, mid);
    let mut req = client.post(url).json(body);
    for (k, v) in &hs {
        req = req.header(k, v);
    }
    match req.send().await {
        Ok(resp) => {
            let st = resp.status().as_u16();
            let ct = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            let text = resp.text().await.unwrap_or_default();
            let head: String = text.chars().take(320).collect();
            println!("[{tag}] HTTP {st} ct={ct}\n    body={head}");
        }
        Err(e) => println!("[{tag}] 请求失败: {e}"),
    }
}
