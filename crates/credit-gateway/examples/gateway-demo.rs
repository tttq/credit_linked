//! 网关独立演示/自检：不依赖 Tauri，直接起网关监听（临时 SQLite）。
use std::net::SocketAddr;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join("credit-gateway-demo");
    let store = Arc::new(credit_core::db::Store::open(&dir.join("demo.sqlite"))?);
    let state = Arc::new(credit_gateway::GatewayState {
        store: store.clone(),
        router: credit_gateway::Router::new(store),
        api_key: String::new(),
    });
    let addr: SocketAddr = "127.0.0.1:8756".parse()?;
    println!("gateway demo listening on http://{addr}");
    credit_gateway::serve(state, addr).await?;
    Ok(())
}
