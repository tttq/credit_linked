#[test]
fn test_normalize_messages() {
    let msgs = serde_json::json!([
        {"role": "system", "content": "你是助手"},
        {"role": "user", "content": "你好"},
        {"role": "developer", "content": "dev"},
        {"role": "user", "content": [{"type": "text", "text": "多模态"}, {"type": "image_url", "image_url": {"url": "x"}}]},
        {"role": "assistant", "content": "回复"}
    ]);
    let (out, system) = credit_gateway::protocol::normalize_messages(msgs.as_array().unwrap());
    assert_eq!(system, "你是助手\ndev");
    assert_eq!(out.len(), 3);
    assert_eq!(out[0]["role"], "user");
    assert_eq!(out[0]["content"][0]["type"], "text");
}

#[test]
fn test_wb_chat_body() {
    let msgs = serde_json::json!([{"role": "user", "content": [{"type": "text", "text": "hi"}]}]);
    let body = credit_gateway::protocol::wb_chat_body(msgs.as_array().unwrap(), "sys", "glm-5.3-flash", Some(1024), Some("high"));
    assert_eq!(body["model"], "glm-5.3-flash");
    assert_eq!(body["stream"], true);
    assert_eq!(body["max_tokens"], 1024);
    assert_eq!(body["reasoning_effort"], "high");
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][0]["content"], "sys");
}

#[test]
fn test_trae_chat_body() {
    let msgs = serde_json::json!([{"role": "user", "content": [{"type": "text", "text": "hi"}]}]);
    let body = credit_gateway::protocol::trae_chat_body(msgs.as_array().unwrap(), "glm-5.2", None, None, true);
    assert_eq!(body["config_name"], "glm-5.2");
    assert_eq!(body["function"], "solo_work_lite");
    assert!(body["session_id"].is_string());
}

#[test]
fn test_trae_version_code() {
    assert_eq!(credit_gateway::protocol::trae_version_code(Some("2.3.76922")), credit_gateway::protocol::TRAE_VERSION_CODE_FALLBACK);
    assert_eq!(credit_gateway::protocol::trae_version_code(Some("76922")), "76922");
    assert_eq!(credit_gateway::protocol::trae_version_code(None), credit_gateway::protocol::TRAE_VERSION_CODE_FALLBACK);
}

#[test]
fn test_parse_openai_chunk() {
    use credit_gateway::upstream::parse_openai_chunk;
    let v = serde_json::json!({"choices": [{"delta": {"content": "你好", "reasoning_content": "思考"}, "finish_reason": null}]});
    let evs = parse_openai_chunk(&v);
    assert_eq!(evs.len(), 2);
    let v2 = serde_json::json!({"error": {"message": "bad", "code": "x"}});
    let evs2 = parse_openai_chunk(&v2);
    assert!(matches!(evs2[0], credit_gateway::upstream::StreamEvent::Error(_, _)));
}

#[test]
fn test_decode_trae_event() {
    use credit_gateway::upstream::decode_trae_event;
    let ev = decode_trae_event("output", r#"{"response": "hi", "reasoning_content": "think"}"#).unwrap();
    assert!(matches!(ev, credit_gateway::upstream::StreamEvent::Reasoning(_)));
    let ev2 = decode_trae_event("token_usage", r#"{"prompt_tokens": 5, "completion_tokens": 7}"#).unwrap();
    assert!(matches!(ev2, credit_gateway::upstream::StreamEvent::Usage { .. }));
    let ev3 = decode_trae_event("error", r#"{"code": 1005}"#).unwrap();
    assert!(matches!(ev3, credit_gateway::upstream::StreamEvent::Error(_, _)));
}

#[test]
fn test_router_rules() {
    // Router 需要 SQLite — 用临时库
    let dir = std::env::temp_dir().join(format!("gw-test-{}", uuid::Uuid::new_v4().simple()));
    let store = std::sync::Arc::new(credit_core::db::Store::open(&dir.join("t.sqlite")).unwrap());
    let router = credit_gateway::Router::new(store);
    // 默认平台
    assert_eq!(router.current_platform(), credit_gateway::protocol::Platform::WorkBuddy);
    // 切换
    router.set_current_platform(credit_gateway::protocol::Platform::Trae);
    assert_eq!(router.current_platform(), credit_gateway::protocol::Platform::Trae);
    // 规则 CRUD
    let rule = router.add_rule(credit_gateway::RouteRule {
        id: String::new(), model_pattern: "workbuddy/glm".into(),
        platform: "workbuddy".into(), account: String::new(),
        weight: 1, enabled: true, created_at: String::new(),
    });
    assert!(!rule.id.is_empty());
    assert_eq!(router.list_rules().len(), 1);
    assert!(router.match_rule("workbuddy/glm-5.3-flash").is_some());
    assert!(router.match_rule("trae/gpt-5.4").is_none());
    assert!(router.delete_rule(&rule.id));
    assert_eq!(router.list_rules().len(), 0);
    // 清理
    let _ = std::fs::remove_dir_all(&dir);
}