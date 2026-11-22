//! 回放探针：把 qdump 抓到的「上游原始 SSE」喂进网关自己的 Qoder 信封泵，
//! 打印每个 StreamEvent —— 用于确认参数是在「解析层」丢的，还是在「聚合/sanitize 层」丢的。
//!
//! 用法：cargo run --offline -p credit-gateway --example qreplay -- <rawfile> [chunkBytes]

use bytes::Bytes;
use credit_gateway::upstream::{parse_qoder_envelope_sse, StreamEvent};
use futures_util::StreamExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("raw file path");
    let chunk: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(512);
    let raw = std::fs::read(&path)?;

    // 模拟网络分片：按固定大小切片整段喂进泵
    let pieces: Vec<Result<Bytes, String>> = raw
        .chunks(chunk)
        .map(|c| Ok(Bytes::copy_from_slice(c)))
        .collect();
    let s = futures_util::stream::iter(pieces);
    let mut evs = parse_qoder_envelope_sse(s);

    let mut n = 0;
    let mut acc = std::collections::BTreeMap::<u32, String>::new();
    while let Some(item) = evs.next().await {
        n += 1;
        match item {
            Ok(StreamEvent::ToolCall { index, id, name, arguments }) => {
                println!(
                    "[{n}] ToolCall index={index} id={:?} name={:?} args={:?}",
                    id, name, arguments
                );
                if let Some(a) = arguments {
                    acc.entry(index).or_default().push_str(&a);
                }
            }
            Ok(StreamEvent::Text(t)) => println!("[{n}] Text {t:?}"),
            Ok(StreamEvent::Reasoning(t)) => println!("[{n}] Reasoning {t:?}"),
            Ok(StreamEvent::Usage { input, output, total }) => {
                println!("[{n}] Usage in={input} out={output} total={total:?}")
            }
            Ok(StreamEvent::Done { finish }) => println!("[{n}] Done finish={finish}"),
            Ok(StreamEvent::Error(m, c)) => println!("[{n}] Error {c}: {m}"),
            Err(e) => println!("[{n}] STREAM ERR {e}"),
        }
    }
    println!("\n=== 事件数 {n} ===");
    for (i, a) in acc {
        println!("index {i} 聚合参数 = {a:?}");
        println!("  是否合法 JSON: {}", serde_json::from_str::<serde_json::Value>(&a).is_ok());
    }
    Ok(())
}
