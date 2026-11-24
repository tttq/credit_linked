//! 独立网关进程：只跑 HTTP 网关（不启 GUI），用于在测试端口上验证真实代码路径。
//!
//! 用法：cargo run -p credit-gateway --example runsrv -- [port]
//!      默认端口 18756；用同一个 DB（%APPDATA%/credit-rs/credit.sqlite）。

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use credit_core::db::Store;
use credit_gateway::protocol::Platform;
use credit_gateway::server::device_salt_init;
use credit_gateway::{GatewayState, Router};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let port: u16 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(18756);

    let store = Arc::new(Store::open_default()?);
    let router = Router::new(store.clone());

    // 平台可由参数 2 指定：runsrv [port] [platform]
    if let Some(p) = std::env::args().nth(2) {
        if Platform::parse(&p).is_some() {
            router.set_current_platform_str(&p);
            println!("[runsrv] gw_platform -> {p}");
        }
    }
    println!("[runsrv] current platform = {}", router.current_platform_raw());

    let api_key = store.get_setting("gatewayApiKey", "").unwrap_or_default();
    let state = Arc::new(GatewayState {
        store: store.clone(),
        router,
        api_key,
        rate_window: Mutex::new(std::collections::VecDeque::new()),
        port,
        device_salt: Mutex::new(device_salt_init()),
        device_token: Mutex::new(None),
        models_cache: Mutex::new(None),
        models_info: Mutex::new(std::collections::HashMap::new()),
    });

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    println!("[runsrv] serving on http://{addr}");
    credit_gateway::serve(state, addr).await?;
    Ok(())
}
