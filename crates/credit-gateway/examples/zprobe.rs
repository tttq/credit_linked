//! 复刻官方 rollout 请求形状 → Start Plan 端点
use serde_json::{json, Value};
use base64::Engine as _;

fn jwt_claims(token: &str) -> Option<Value> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 { return None; }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[1].trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let live = credit_core::adapters::zcode_local::read_live_auth().ok_or_else(|| anyhow::anyhow!("no live"))?;
    let jwt = live.zcode_jwt.clone().unwrap_or_default();
    let mid = live.device_mid.clone();
    let uid = jwt_claims(&jwt).and_then(|c| c.get("user_id").and_then(|u| u.as_str()).map(|s| s.to_string())).unwrap_or_default();
    println!("uid={uid} mid={mid}");

    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(60)).build()?;
    let url = "https://zcode.z.ai/api/v1/zcode-plan/anthropic/v1/messages";
    // 官方 rollout 请求体形状（model-io 真实记录）
    let body = json!({
        "model": "GLM-5.3-Flash",
        "max_tokens": 128000,
        "metadata": { "user_id": uid },
        "system": [
            {"type":"text","text":"You are a helpful assistant","cache_control":{"type":"ephemeral"}}
        ],
        "messages": [{"role":"user","content":[{"type":"text","text":"reply ok"}]}],
        "stream": true,
        "thinking": {"type":"enabled"},
        "output_config": {"effort": "high"}
    });
    let resp = client.post(url)
        .header("Authorization", format!("Bearer {jwt}"))
        .header("anthropic-version", "2023-06-01")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("User-Agent", "ZCode/3.18.2")
        .header("HTTP-Referer", "https://zcode.z.ai")
        .header("X-Title", "Z Code@electron")
        .header("X-ZCode-App-Version", "3.18.2")
        .header("X-Release-Channel", "production")
        .header("X-Client-Language", "zh-CN")
        .header("X-Client-Timezone", "Asia/Shanghai")
        .header("x-zcode-agent", "glm")
        .header("X-Platform", "win32-x64")
        .header("X-Os-Category", "windows")
        .header("X-Os-Version", "10.0.26300")
        .header("x-request-id", uuid::Uuid::new_v4().to_string())
        .header("x-zcode-session-type", "main")
        .header("x-zcode-trace-id", uuid::Uuid::new_v4().to_string())
        .header("x-query-id", uuid::Uuid::new_v4().to_string())
        .header("x-zcode-query-source", "main_turn")
        .header("x-session-id", uuid::Uuid::new_v4().to_string())
        .header("X-Device-Mid", &mid)
        .json(&body)
        .send().await?;
    let st = resp.status().as_u16();
    let ct = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let t = resp.text().await.unwrap_or_default();
    println!("[plan+official-shape] HTTP {st} ct={ct}\n{}", t.chars().take(400).collect::<String>());
    Ok(())
}
