//! 回归：Qoder 的 tool_call 参数分片在「首帧/收尾帧带 `arguments:""`」时不能被污染。
//!
//! 线上故障链（DSH + Qoder gfmodel，GLM-5.3-Flash）：
//!   1. 上游 tool_call 首帧 `{"arguments":"","name":"skill"}`、收尾帧再发一次 `arguments:""`；
//!   2. `tool_args_str` 把空串落进兜底分支 → 变成字面量 `""` 分片；
//!   3. 客户端拼出 `""{"name": "novel-comic-drama"}""` → 解析失败 →
//!      DSH 侧工具报 `invalid arguments: missing required property "name"`（模型重试 5 次后放弃）。
use bytes::Bytes;
use credit_gateway::upstream::{parse_openai_chunk, parse_qoder_envelope_sse, StreamEvent};
use futures_util::StreamExt;
use serde_json::json;

/// 包一层 Qoder 外层信封（线上形态：data:{headers,body,statusCode}）
fn envelope(inner: &str) -> String {
    let env = json!({
        "headers": { "Content-Type": ["application/json"] },
        "body": inner,
        "statusCodeValue": 200,
        "statusCode": "OK",
    });
    format!("data:{env}\n\n")
}

/// 内层 chunk：tool_call 增量帧
fn tool_call_chunk(arguments: &str, id: &str, name: Option<&str>) -> String {
    let mut func = json!({ "arguments": arguments });
    if let Some(n) = name {
        func["name"] = json!(n);
    }
    json!({
        "choices": [{ "delta": { "tool_calls": [{
            "function": func, "id": id, "index": 0, "type": "function"
        }]}, "index": 0 }],
        "id": "chatcmpl-test", "model": "auto", "object": "chat.completion.chunk",
    })
    .to_string()
}

#[test]
fn empty_argument_fragment_maps_to_none() {
    // 空串分片必须是「没有参数」，不能变成字面量 `""`
    let chunk = json!({
        "choices": [{ "delta": { "tool_calls": [{
            "function": { "arguments": "", "name": "skill" }, "id": "call_1", "index": 0
        }]}, "index": 0 }]
    });
    let evs = parse_openai_chunk(&chunk);
    match &evs[0] {
        StreamEvent::ToolCall { arguments, name, .. } => {
            assert!(arguments.is_none(), "空串参数分片应被丢弃，实际 {arguments:?}");
            assert_eq!(name.as_deref(), Some("skill"));
        }
        other => panic!("期望 ToolCall，得到 {other:?}"),
    }
}

#[tokio::test]
async fn qoder_fragment_stream_reassembles_valid_json() {
    // 完全复刻实测帧序列（arguments 在每个分片里逐字累加）
    let frames = [
        tool_call_chunk("", "call_ff1cb4c3df27468488cbf3fb", Some("skill")),
        tool_call_chunk(r#"{"name": "#, "", None),
        tool_call_chunk(r#""novel-com"#, "", None),
        tool_call_chunk("ic-d", "", None),
        tool_call_chunk("rama", "", None),
        tool_call_chunk(r#"""#, "", None),
        tool_call_chunk("}", "", None),
        tool_call_chunk("", "", None),
        json!({
            "choices": [{ "delta": { "content": "" }, "finish_reason": "tool_calls", "index": 0 }]
        })
        .to_string(),
    ];
    let raw: String = frames.iter().map(|f| envelope(f)).collect();

    // 模拟网络分片：任意切块都不能丢参数
    for chunk in [7usize, 64, 4096] {
        let pieces: Vec<Result<Bytes, String>> = raw
            .as_bytes()
            .chunks(chunk)
            .map(|c| Ok(Bytes::copy_from_slice(c)))
            .collect();
        let mut evs = parse_qoder_envelope_sse(futures_util::stream::iter(pieces));
        let mut args = String::new();
        let mut name = String::new();
        let mut done = false;
        while let Some(item) = evs.next().await {
            match item.expect("流不应报错") {
                StreamEvent::ToolCall { arguments, name: n, .. } => {
                    if let Some(a) = arguments {
                        args.push_str(&a);
                    }
                    if let Some(n) = n {
                        name.push_str(&n);
                    }
                }
                StreamEvent::Done { .. } => done = true,
                _ => {}
            }
        }
        assert!(done, "chunk={chunk} 缺少 Done 事件");
        assert_eq!(name, "skill");
        let v: serde_json::Value =
            serde_json::from_str(&args).unwrap_or_else(|e| panic!("chunk={chunk} 参数非法: {args:?} ({e})"));
        assert_eq!(v["name"], "novel-comic-drama", "chunk={chunk}");
    }
}
