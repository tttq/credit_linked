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
    let body = credit_gateway::protocol::wb_chat_body(msgs.as_array().unwrap(), "sys", "glm-5.3-flash", Some(1024), Some("high"), "");
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
    let body = credit_gateway::protocol::trae_chat_body(msgs.as_array().unwrap(), "glm-5.2", "solo_work_lite", None, None, true);
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
/// 中转链路全平台覆盖：Platform 的 9 个变体映射到 6 个上游通道（trae 三态、wb 双态、
/// codearts 双态共用 builder）。平台 → 上游的分发是 server.rs dispatch 的穷尽 match，
/// 编译期保证无遗漏；本测试保证每个通道的 body 构造 + 工具协议挂载在运行时可用
/// （Agent 中转三要素：stream=true、工具定义可挂载、消息结构合法）。
#[test]
fn test_all_platforms_relay_body_and_tools() {
    let msgs = serde_json::json!([{ "role": "user", "content": [{ "type": "text", "text": "hi" }] }]);
    let tools = serde_json::json!([
        { "type": "function", "function": { "name": "read_file", "description": "read", "parameters": { "type": "object", "properties": { "path": { "type": "string" } } } } }
    ]).as_array().unwrap().clone();
    let choice = serde_json::json!("auto");

    // workbuddy / workbuddy-intl（OpenAI 风格工具）
    let mut b = credit_gateway::protocol::wb_chat_body(msgs.as_array().unwrap(), "sys", "glm-5.3-flash", Some(512), Some("high"), "u1");
    assert_eq!(b["stream"], true);
    credit_gateway::protocol::attach_openai_tools(&mut b, &tools, Some(&choice));
    assert_eq!(b["tools"][0]["function"]["name"], "read_file");

    // trae / trae-enterprise / trae-intl（tools[].function.parameters 需序列化成字符串）
    let mut t = credit_gateway::protocol::trae_chat_body(msgs.as_array().unwrap(), "glm-5.2", "solo_work_lite", None, None, true);
    credit_gateway::protocol::attach_trae_tools(&mut t, &tools);
    assert!(t["tools"][0]["function"]["parameters"].is_string());

    // zcode（Anthropic 风格工具 + input_schema）
    let mut z = credit_gateway::protocol::zcode_chat_body(msgs.as_array().unwrap(), "sys", "model-x", None, true);
    credit_gateway::protocol::attach_anthropic_tools(&mut z, &tools, Some(&choice));
    assert_eq!(z["tools"][0]["name"], "read_file");
    assert_eq!(z["tools"][0]["input_schema"]["type"], "object");

    // minimax（Anthropic 风格）
    let mut m = credit_gateway::protocol::minimax_chat_body(msgs.as_array().unwrap(), "sys", "model-x", None);
    credit_gateway::protocol::attach_anthropic_tools(&mut m, &tools, Some(&choice));
    assert_eq!(m["tools"][0]["name"], "read_file");

    // qoder（OpenAI 风格）
    let mut q = credit_gateway::protocol::qoder_chat_body(msgs.as_array().unwrap(), "model-y");
    credit_gateway::protocol::attach_openai_tools(&mut q, &tools, Some(&choice));
    assert_eq!(q["tools"][0]["function"]["name"], "read_file");

    // codearts / codearts-desktop（OpenAI 风格）
    let mut c = credit_gateway::protocol::codearts_chat_body(msgs.as_array().unwrap(), "sys", "model-z", Some(256), Some("medium"));
    credit_gateway::protocol::attach_openai_tools(&mut c, &tools, Some(&choice));
    assert_eq!(c["tools"][0]["function"]["name"], "read_file");
}

/// 上游不发结束帧就断开（长任务常见故障）：解析器不 panic，工具分片事件完整保留；
/// 参数修复与 finish_reason 补发由网关收尾流（stream_response 的 close 流）负责。
#[test]
fn test_stream_ends_without_done_frame_keeps_tool_events() {
    use credit_gateway::upstream::{parse_openai_chunk, StreamEvent};
    let v = serde_json::json!({"choices": [{"delta": {"tool_calls": [
        {"index": 0, "id": "call_1", "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":"}}
    ]}, "finish_reason": null}]});
    let evs = parse_openai_chunk(&v);
    match &evs[0] {
        StreamEvent::ToolCall { index, id, name, arguments } => {
            assert_eq!(*index, 0);
            assert_eq!(id.as_deref(), Some("call_1"));
            assert_eq!(name.as_deref(), Some("read_file"));
            assert_eq!(arguments.as_deref(), Some("{\"path\":"));
        }
        other => panic!("expected tool call, got {other:?}"),
    }
}
