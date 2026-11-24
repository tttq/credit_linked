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
    let body = credit_gateway::protocol::trae_chat_body(msgs.as_array().unwrap(), "", "glm-5.2", "solo_work_lite", None, None, true);
    assert_eq!(body["config_name"], "glm-5.2");
    assert_eq!(body["function"], "solo_work_lite");
    assert!(body["session_id"].is_string());
}

/// 回归（2026-11-24）：Trae/Qoder 曾把 system 提示词整体丢弃
/// —— 导致 Agent 拿不到「项目目录 / 必须用工具」指令，
/// 模型退化成纯文本作答（不执行工具、目录猜错）。
/// 两者都必须把 system 还原成 messages 数组的**首条** `role:"system"`。
#[test]
fn test_trae_and_qoder_keep_system_prompt_as_first_message() {
    let msgs = serde_json::json!([{"role": "user", "content": [{"type": "text", "text": "hi"}]}]);
    let sys = "You are running in E:\\proj. Use tools to inspect the filesystem.";

    // Trae
    let t = credit_gateway::protocol::trae_chat_body(msgs.as_array().unwrap(), sys, "glm-5.2", "solo_work_lite", None, None, false);
    assert_eq!(t["messages"][0]["role"], "system");
    assert_eq!(t["messages"][0]["content"], sys);
    // system 必须在 user 之前
    assert_eq!(t["messages"][1]["role"], "user");

    // Qoder
    let q = credit_gateway::protocol::qoder_chat_body(msgs.as_array().unwrap(), sys, "model-y");
    assert_eq!(q["messages"][0]["role"], "system");
    assert_eq!(q["messages"][0]["content"], sys);
    assert_eq!(q["messages"][1]["role"], "user");

    // 空 system 时不应插入空条目
    let q0 = credit_gateway::protocol::qoder_chat_body(msgs.as_array().unwrap(), "", "model-y");
    assert_eq!(q0["messages"][0]["role"], "user");
    let q0w = credit_gateway::protocol::qoder_chat_body(msgs.as_array().unwrap(), "   ", "model-y");
    assert_eq!(q0w["messages"][0]["role"], "user");
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
    let mut t = credit_gateway::protocol::trae_chat_body(msgs.as_array().unwrap(), "sys", "glm-5.2", "solo_work_lite", None, None, true);
    credit_gateway::protocol::attach_trae_tools(&mut t, &tools);
    assert!(t["tools"][0]["function"]["parameters"].is_string());
    assert_eq!(t["messages"][0]["role"], "system");

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
    let mut q = credit_gateway::protocol::qoder_chat_body(msgs.as_array().unwrap(), "sys", "model-y");
    credit_gateway::protocol::attach_openai_tools(&mut q, &tools, Some(&choice));
    assert_eq!(q["tools"][0]["function"]["name"], "read_file");
    assert_eq!(q["messages"][0]["role"], "system");

    // codearts / codearts-desktop（OpenAI 风格）
    let mut c = credit_gateway::protocol::codearts_chat_body(msgs.as_array().unwrap(), "sys", "model-z", Some(256), Some("medium"));
    credit_gateway::protocol::attach_openai_tools(&mut c, &tools, Some(&choice));
    assert_eq!(c["tools"][0]["function"]["name"], "read_file");
}

/// 上游不发结束帧就断开（长任务常见故障）：解析器不 panic，工具分片事件完整保留；
/// 参数修复与 finish_reason 补发由网关收尾流（stream_response 的 close 流）负责。
/// ZCode Start Plan 端点的「仅官方客户端」闸门（code=3012）回归测试。
///
/// 2026-11-19 实测：服务端同时要求
///   ① 请求头 `X-ZCode-App-Version` == 当前客户端版本（3.18.2）
///   ② 请求体 `system` 前两块是官方 CLI 固定文本块（顺序不可颠倒）
/// 任一不满足 → HTTP 405 / code=3012。缺 system / 少一块 / 顺序颠倒都已被实测拒绝。
#[test]
fn test_zcode_start_plan_system_blocks_gate() {
    use credit_gateway::protocol::{
        zcode_plan_chat_headers, zcode_start_plan_system_blocks,
        ZCODE_SYSTEM_CLI_PREFIX, ZCODE_SYSTEM_IDENTITY,
    };

    // ① system 块：前两块必须是官方固定文本
    let blocks = zcode_start_plan_system_blocks("");
    assert_eq!(blocks.len(), 2, "无用户 system 时应只有官方两块");
    assert_eq!(blocks[0]["text"], ZCODE_SYSTEM_CLI_PREFIX);
    assert_eq!(blocks[1]["text"], ZCODE_SYSTEM_IDENTITY);
    assert_eq!(blocks[0]["cache_control"]["type"], "ephemeral");
    assert_eq!(blocks[0]["type"], "text");

    // 用户 system 追加为第 3 块（不破坏前两块顺序）
    let blocks3 = zcode_start_plan_system_blocks("你是助手");
    assert_eq!(blocks3.len(), 3);
    assert_eq!(blocks3[0]["text"], ZCODE_SYSTEM_CLI_PREFIX);
    assert_eq!(blocks3[1]["text"], ZCODE_SYSTEM_IDENTITY);
    assert_eq!(blocks3[2]["text"], "你是助手");

    // 空白用户 system 不应产生空块
    assert_eq!(zcode_start_plan_system_blocks("   ").len(), 2);

    // ② 请求头：版本头必须是 3.18.2，且带认证
    let hs = zcode_plan_chat_headers("JWT_TOKEN", "MID");
    let get = |k: &str| hs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
    assert_eq!(get("x-zcode-app-version"), "3.18.2");
    assert_eq!(get("authorization"), "Bearer JWT_TOKEN");
    assert_eq!(get("x-api-key"), "JWT_TOKEN");
    assert_eq!(get("x-device-mid"), "MID");
    assert_eq!(get("anthropic-version"), "2023-06-01");

    // ③ 身份块文本不能为空，且 Identity 块必须以换行开头（官方 lJa 的拼接语义）
    assert!(!ZCODE_SYSTEM_CLI_PREFIX.is_empty());
    assert!(ZCODE_SYSTEM_IDENTITY.starts_with('\n'));
    assert!(ZCODE_SYSTEM_IDENTITY.contains("# Harness"));
}

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

// ===== OpenAI → Anthropic 消息转换（MiniMax/ZCode 工具回合） =====

#[test]
fn test_to_anthropic_messages_tool_round() {
    use credit_gateway::protocol::to_anthropic_messages;
    let msgs = serde_json::json!([
        {"role": "user", "content": "查北京天气"},
        {"role": "assistant", "content": "", "tool_calls": [
            {"id": "call_1", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":\"北京\"}"}}
        ]},
        {"role": "tool", "tool_call_id": "call_1", "content": "{\"temp\":18}"}
    ]);
    let (out, sys) = to_anthropic_messages(msgs.as_array().unwrap(), "sys");
    assert_eq!(sys, "sys");
    assert_eq!(out.len(), 3);
    assert_eq!(out[0]["role"], "user");
    assert_eq!(out[1]["role"], "assistant");
    // assistant.tool_calls → tool_use（input 必须是对象）
    assert_eq!(out[1]["content"][0]["type"], "tool_use");
    assert_eq!(out[1]["content"][0]["id"], "call_1");
    assert_eq!(out[1]["content"][0]["name"], "get_weather");
    assert_eq!(out[1]["content"][0]["input"]["city"], "北京");
    // tool → user.tool_result（tool_use_id 与上游一致）
    assert_eq!(out[2]["role"], "user");
    assert_eq!(out[2]["content"][0]["type"], "tool_result");
    assert_eq!(out[2]["content"][0]["tool_use_id"], "call_1");
}

#[test]
fn test_to_anthropic_messages_merges_tool_results_and_fills_missing_id() {
    use credit_gateway::protocol::to_anthropic_messages;
    // 两个连续 tool 消息必须合并进同一条 user 消息；第一个缺 tool_call_id 时按顺序补配
    let msgs = serde_json::json!([
        {"role": "assistant", "content": "", "tool_calls": [
            {"id": "call_a", "type": "function", "function": {"name": "f1", "arguments": "{}"}},
            {"id": "call_b", "type": "function", "function": {"name": "f2", "arguments": "{}"}}
        ]},
        {"role": "tool", "content": "r1"},
        {"role": "tool", "tool_call_id": "call_b", "content": "r2"}
    ]);
    let (out, _) = to_anthropic_messages(msgs.as_array().unwrap(), "");
    // 首条被补 (continue) user，随后 assistant，两条 tool_result 合并为一条 user
    assert_eq!(out.len(), 3);
    let results = out[2]["content"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["tool_use_id"], "call_a");
    assert_eq!(results[1]["tool_use_id"], "call_b");
}

#[test]
fn test_anthropic_sse_tool_events() {
    use credit_gateway::protocol::parse_anthropic_chunk;
    use credit_gateway::upstream::StreamEvent;
    let start = serde_json::json!({"type":"content_block_start","index":1,
        "content_block":{"type":"tool_use","id":"call_x","name":"read_file"}});
    let evs = parse_anthropic_chunk(&start);
    match &evs[0] {
        StreamEvent::ToolCall { index, id, name, .. } => {
            assert_eq!(*index, 0);
            assert_eq!(id.as_deref(), Some("call_x"));
            assert_eq!(name.as_deref(), Some("read_file"));
        }
        other => panic!("expected tool start, got {other:?}"),
    }
    // 参数分片：input_json_delta.partial_json → arguments 增量
    let delta = serde_json::json!({"type":"content_block_delta","index":1,
        "delta":{"type":"input_json_delta","partial_json":"{\"path\":"}});
    match &parse_anthropic_chunk(&delta)[0] {
        StreamEvent::ToolCall { index, arguments, .. } => {
            assert_eq!(*index, 0);
            assert_eq!(arguments.as_deref(), Some("{\"path\":"));
        }
        other => panic!("expected args delta, got {other:?}"),
    }
    // stop_reason=tool_use → finish_reason=tool_calls
    let stop = serde_json::json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}});
    let evs2 = parse_anthropic_chunk(&stop);
    assert!(evs2.iter().any(|e| matches!(e, StreamEvent::Done { finish } if finish == "tool_calls")));
    assert!(evs2.iter().any(|e| matches!(e, StreamEvent::Usage { .. })));
}

// ===== Qoder 外层信封错误帧（此前被静默丢弃 → 客户端只看到空流/502） =====

#[test]
fn test_qoder_envelope_surfaces_invalid_model_error() {
    // 实测帧：HTTP 200 + 外层 statusCode=BAD_REQUEST，body 是 invalid_model_error
    let frame = serde_json::json!({
        "headers": {"Content-Type": ["application/json"]},
        "body": "{\"code\":\"invalid_model_error\",\"message\":\"Unsupported model \\\"performance\\\"\",\"request_id\":\"x\",\"type\":\"invalid_model_error\"}\n",
        "statusCodeValue": 400,
        "statusCode": "BAD_REQUEST"
    }).to_string();
    match credit_gateway::upstream::qoder_envelope_event(&frame) {
        Some(credit_gateway::upstream::StreamEvent::Error(msg, code)) => {
            assert_eq!(code, "invalid_model_error");
            assert!(msg.contains("Unsupported model"));
        }
        other => panic!("expected error event, got {other:?}"),
    }
    // 正常 chunk 仍按 OpenAI 解析
    let ok = serde_json::json!({
        "body": "{\"choices\":[{\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}",
        "statusCodeValue": 200, "statusCode": "OK"
    }).to_string();
    match credit_gateway::upstream::qoder_envelope_event(&ok) {
        Some(credit_gateway::upstream::StreamEvent::Text(t)) => assert_eq!(t, "hi"),
        other => panic!("expected text event, got {other:?}"),
    }
}

#[test]
fn test_qoder_envelope_inner_done_is_stream_end_not_error() {
    // 用户实测故障：Qoder 上游 HTTP 200，外层信封的 body 直接是 SSE 终止标记 "[DONE]"。
    // 旧实现把它当「非 JSON」→ 产出 Error 事件 → 整个响应被判失败，Qoder 完全不可用。
    // 正确语义：这是**正常结束**。
    let frame = serde_json::json!({
        "headers": {"Content-Type": ["text/event-stream"]},
        "body": "[DONE]",
        "statusCodeValue": 200,
        "statusCode": "OK"
    }).to_string();
    match credit_gateway::upstream::qoder_envelope_event(&frame) {
        Some(credit_gateway::upstream::StreamEvent::Done { .. }) => {}
        other => panic!("内层 [DONE] 应转成 Done（正常结束），得到 {other:?}"),
    }

    // 内层 body 是「嵌套 SSE 文本」时也必须按帧解析，而不是报非 JSON
    let nested = serde_json::json!({
        "body": "data: {\"choices\":[{\"delta\":{\"content\":\"你好\"},\"finish_reason\":null}]}\n\ndata: [DONE]\n\n",
        "statusCodeValue": 200, "statusCode": "OK"
    }).to_string();
    match credit_gateway::upstream::qoder_envelope_event(&nested) {
        Some(credit_gateway::upstream::StreamEvent::Text(t)) => assert_eq!(t, "你好"),
        other => panic!("嵌套 SSE 首个内容帧应解析成 Text，得到 {other:?}"),
    }
    // 正文里含 "data:"（如 base64 data URL）的普通 chunk 不能被误判成 SSE 空流
    let dat = serde_json::json!({
        "body": "{\"choices\":[{\"delta\":{\"content\":\"data:image/png;base64,AAA\"},\"finish_reason\":null}]}",
        "statusCodeValue": 200, "statusCode": "OK"
    }).to_string();
    match credit_gateway::upstream::qoder_envelope_event(&dat) {
        Some(credit_gateway::upstream::StreamEvent::Text(t)) => {
            assert!(t.contains("data:image/png"), "含 data: 的正文应原样当作 Text，得到 {t:?}")
        }
        other => panic!("含 data: 的普通 chunk 应解析为 Text，得到 {other:?}"),
    }

    // 外层 data 帧直接是 [DONE]（非信封 JSON）也必须当正常结束，不能丢帧后无 Done
    match credit_gateway::upstream::qoder_envelope_event("[DONE]") {
        Some(credit_gateway::upstream::StreamEvent::Done { .. }) => {}
        other => panic!("外层 [DONE] 应转成 Done，得到 {other:?}"),
    }
}

#[test]
fn test_qoder_envelope_truly_invalid_body_still_errors() {
    // 真正的非 JSON（上游网关错误页）仍必须当错误报出来，不能被 [DONE] 修复误伤
    let frame = serde_json::json!({
        "body": "<html>502 Bad Gateway</html>",
        "statusCodeValue": 502, "statusCode": "BAD_GATEWAY"
    }).to_string();
    match credit_gateway::upstream::qoder_envelope_event(&frame) {
        Some(credit_gateway::upstream::StreamEvent::Error(_, code)) => {
            assert_eq!(code, "qoder_bad_body");
        }
        other => panic!("HTML 错误页仍应报 Error，得到 {other:?}"),
    }
}

#[test]
fn test_to_anthropic_messages_synthesizes_missing_tool_result() {
    use credit_gateway::protocol::to_anthropic_messages;
    // 截断/中断后历史里 assistant 有 tool_use 但缺 tool 结果：官方客户端会补 is_error 占位
    let msgs = serde_json::json!([
        {"role": "assistant", "content": "", "tool_calls": [
            {"id": "call_z", "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"a\"}"}}
        ]}
    ]);
    let (out, _) = to_anthropic_messages(msgs.as_array().unwrap(), "");
    // assistant 开头会补一条 user 占位（(continue)），随后 assistant，最后补齐的 tool_result
    assert_eq!(out.len(), 3);
    assert_eq!(out[0]["role"], "user");
    assert_eq!(out[1]["role"], "assistant");
    assert_eq!(out[2]["role"], "user");
    assert_eq!(out[2]["content"][0]["type"], "tool_result");
    assert_eq!(out[2]["content"][0]["tool_use_id"], "call_z");
    assert_eq!(out[2]["content"][0]["is_error"], true);
}

#[test]
fn test_to_anthropic_messages_drops_orphan_tool_result() {
    use credit_gateway::protocol::to_anthropic_messages;
    // tool 结果的 id 不在紧邻 assistant 的 tool_use 里（历史断裂）：必须丢弃，避免上游 unexpected id
    let msgs = serde_json::json!([
        {"role": "assistant", "content": "", "tool_calls": [
            {"id": "call_ok", "type": "function", "function": {"name": "f", "arguments": "{}"}}
        ]},
        {"role": "tool", "tool_call_id": "call_orphan", "content": "bogus"}
    ]);
    let (out, _) = to_anthropic_messages(msgs.as_array().unwrap(), "");
    let has_orphan = out.iter().any(|m| {
        m.get("content").and_then(|c| c.as_array()).map(|a| {
            a.iter().any(|b| b.get("tool_use_id").and_then(|i| i.as_str()) == Some("call_orphan"))
        }).unwrap_or(false)
    });
    assert!(!has_orphan, "孤儿 tool_result 必须丢弃");
    // 同时真正缺失的 call_ok 被补上占位
    let has_synth = out.iter().any(|m| {
        m.get("content").and_then(|c| c.as_array()).map(|a| {
            a.iter().any(|b| b.get("tool_use_id").and_then(|i| i.as_str()) == Some("call_ok"))
        }).unwrap_or(false)
    });
    assert!(has_synth);
}

#[test]
fn test_to_anthropic_messages_tool_result_is_first_block() {
    use credit_gateway::protocol::to_anthropic_messages;
    // 用户文本与工具结果混合时，tool_result 必须是 user 消息首个块
    let msgs = serde_json::json!([
        {"role": "assistant", "content": "", "tool_calls": [
            {"id": "call_1", "type": "function", "function": {"name": "f", "arguments": "{}"}}
        ]},
        {"role": "tool", "tool_call_id": "call_1", "content": "r1"},
        {"role": "user", "content": "继续"}
    ]);
    let (out, _) = to_anthropic_messages(msgs.as_array().unwrap(), "");
    let tool_msg = out.iter().find(|m| {
        m.get("content").and_then(|c| c.as_array())
            .map(|a| a.first().and_then(|b| b.get("type")).and_then(|t| t.as_str()) == Some("tool_result"))
            .unwrap_or(false)
    }).expect("应有以 tool_result 开头的 user 消息");
    let arr = tool_msg["content"].as_array().unwrap();
    assert_eq!(arr[0]["type"], "tool_result");
}
