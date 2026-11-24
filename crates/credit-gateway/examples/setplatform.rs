//! 切换网关当前平台（写 settings.gw_platform）。
//!
//! 用法：cargo run -p credit-gateway --example setplatform -- [platform]
//!      不带参数则只打印当前平台与全部账号概览。

use credit_core::db::Store;

fn main() -> anyhow::Result<()> {
    let store = Store::open_default()?;
    let arg = std::env::args().nth(1);

    match arg {
        Some(p) => {
            if credit_gateway::protocol::Platform::parse(&p).is_none() {
                anyhow::bail!("unknown platform: {p}");
            }
            store.set_setting("gw_platform", &p)?;
            println!("gw_platform -> {p}");
        }
        None => {
            let cur = store.get_setting("gw_platform", "workbuddy").unwrap_or_default();
            println!("current gw_platform = {cur}");
        }
    }

    // 概览：各平台账号数 + 模型目录里的 qoder/trae 模型
    println!("\n-- accounts --");
    for (prov, uid, name, _, _) in store.list_accounts()? {
        println!("  {prov:<16} {uid:<40} {name}");
    }
    Ok(())
}
