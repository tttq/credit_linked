//! 凭据维护诊断：实测「扫描本机登录态同步」与「全平台自动续期」。
//!
//! 用法：cargo run -p credit-gateway --example maint -- [sync|renew|all]
//!   sync  = 只做本机登录态同步（Service::sync_from_local）
//!   renew = 只做到期续期巡检（Service::renew_expiring，提前 10 分钟）
//!   all   = 两者都做（默认，等价于网关后台维护任务的一轮）
use std::sync::Arc;

use credit_core::db::Store;
use credit_core::service::Service;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cmd = std::env::args().nth(1).unwrap_or_else(|| "all".into());
    let store = Arc::new(Store::open_default()?);
    let svc = Service { store: store.clone() };

    // 清理 uid 为空的脏行（accounts 主键是 (provider, uid)，空 uid 会挤掉真实账号）
    let pruned = svc.prune_dirty_accounts();
    if pruned > 0 {
        println!("已清理 uid 为空的脏账号 {pruned} 条");
    }

    println!("=== 库内账号 ===");
    for (p, u, n, _s, auth) in store.list_accounts()? {
        let exp = serde_json::from_str::<serde_json::Value>(&auth).ok()
            .and_then(|v| v.get("expires_at").and_then(|e| e.as_str()).map(|s| s.to_string()))
            .unwrap_or_else(|| "-".into());
        println!("  {p}\t{u}\t{n}\texpires={exp}");
    }

    if cmd == "sync" || cmd == "all" {
        println!("\n=== 扫描本机登录态同步 ===");
        for (p, _u, r) in svc.sync_from_local().await {
            println!("  {p}: {r}");
        }
    }

    if cmd == "renew" || cmd == "all" {
        // 可选用第二个参数指定提前窗口（秒），默认 600；调大可强制触发续期链路做验证
        let within: i64 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(600);
        println!("\n=== 到期自动续期（提前 {within}s） ===");
        let out = svc.renew_expiring(within).await;
        if out.is_empty() {
            println!("  （没有即将过期的账号）");
        }
        for (p, u, r) in out {
            println!("  {p}:{u} -> {r}");
        }
    }

    println!("\n=== 同步后账号 ===");
    for (p, u, n, _s, auth) in store.list_accounts()? {
        let exp = serde_json::from_str::<serde_json::Value>(&auth).ok()
            .and_then(|v| v.get("expires_at").and_then(|e| e.as_str()).map(|s| s.to_string()))
            .unwrap_or_else(|| "-".into());
        println!("  {p}\t{u}\t{n}\texpires={exp}");
    }
    Ok(())
}
