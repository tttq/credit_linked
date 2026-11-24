// Test ZCode Client Simulator Integration
// Run with: cargo test --package credit-core zcode_client_simulator_tests

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_zcode_chat_stream_integration() {
        // 模拟一个基本的对话请求
        let body = json!({
            "model": "GLM-5.3-Flash",
            "messages": [
                {"role": "user", "content": "你好"}
            ],
            "max_tokens": 2000
        });

        // 创建测试凭据
        let mut meta = serde_json::Map::new();
        meta.insert("zcodeJwt".to_string(), json!("test_jwt_token"));
        meta.insert("deviceMid".to_string(), json!("test-device-mid"));
        
        let cred = Cred {
            provider: "zcode".to_string(),
            token: "test_token".to_string(),
            uid: "test_uid".to_string(),
            name: "Test User".to_string(),
            refresh_token: None,
            expires_at: None,
            meta: serde_json::Value::Object(meta),
        };

        // 尝试调用 chat_stream
        // 注意：这可能需要实际的凭据才能成功
        let result = zcode_chat_stream(&cred, body).await;
        
        // 检查错误类型
        match result {
            Ok(_) => println!("✓ Chat stream succeeded"),
            Err(e) => {
                // 预期会失败，因为缺少有效的凭据
                println!("Expected error (no valid credentials): {}", e);
                assert!(e.contains("Failed to load") || 
                       e.contains("No zcodejwttoken") || 
                       e.contains("Device Mid not found") ||
                       e.contains("Failed to get fresh access token"));
            }
        }
    }

    #[tokio::test]
    async fn test_simulator_credentials_loading() {
        // 测试凭据加载逻辑
        let simulator = get_simulator();
        
        // 清除之前的状态
        {
            let mut sim = simulator.lock().unwrap();
            *sim = ZcodeClientSimulator::default();
        }

        // 尝试加载凭据（应该失败，因为没有本地客户端）
        let result = {
            let mut sim = simulator.lock().unwrap();
            sim.load_from_local_client()
        };

        // 预期会失败
        assert!(result.is_err());
        println!("Expected error when loading from non-existent local client");
    }
}
