// 签到实测探针。
//   ckprobe [provider ...]        直接调 adapter 的 state/claim，打印原始响应
//   ckprobe svc [provider ...]    走 Service::checkin_account（与 UI 按钮完全同一条路径，含自愈/续期）
//   ckprobe all / ckprobe svc all 全部有账号的平台
use credit_core::adapters::*;
use credit_core::db::Store;
use credit_core::service::{adapter_for, Service};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let svc_mode = args.iter().any(|a| a == "svc");
    let filters: Vec<String> = args.iter().filter(|a| *a != "svc").cloned().collect();
    let want_all = filters.is_empty() || filters.iter().any(|a| a == "all");

    let store_arc = match Store::open_default() {
        Ok(s) => std::sync::Arc::new(s),
        Err(e) => {
            eprintln!("open store: {e}");
            return;
        }
    };
    let rows = store_arc.list_accounts().unwrap_or_default();

    if svc_mode {
        let svc = Service { store: store_arc.clone() };
        let pruned = svc.prune_dirty_accounts();
        if pruned > 0 {
            println!("[prune] 清理脏账号行 {pruned} 条（uid 为空）");
        }
        let rows = store_arc.list_accounts().unwrap_or_default();
        for (provider, uid, name, _s, _auth) in rows {
            if !want_all && !filters.iter().any(|a| a == &provider) {
                continue;
            }
            if uid.trim().is_empty() {
                println!("[skip] {provider} / {name} uid 为空（脏行）");
                continue;
            }
            match tokio::time::timeout(
                std::time::Duration::from_secs(75),
                svc.checkin_account(&provider, &uid),
            )
            .await
            {
                Ok(Ok(o)) => println!(
                    "[svc] {:<18} {:<20} status={:<14} amount={:?} msg={:?}",
                    provider, name, o.status, o.amount, o.message
                ),
                Ok(Err(e)) => println!("[svc] {:<18} {:<20} ERR {e}", provider, name),
                Err(_) => println!("[svc] {:<18} {:<20} 超时(75s)", provider, name),
            }
        }
        return;
    }

    for (provider, uid, name, _s, auth) in rows {
        if !want_all && !filters.iter().any(|a| a == &provider) {
            continue;
        }
        let Ok(cred) = serde_json::from_str::<Cred>(&auth) else { continue };
        let Some(adapter) = adapter_for(&provider) else {
            println!("\n=== {provider} / {name} === 无适配器，跳过");
            continue;
        };
        println!("\n========== {provider} / {name} (uid={uid}) ==========");
        match adapter.state(&cred).await {
            Ok(st) => {
                println!("[state] checked={} claimable={}", st.checked, st.claimable);
                println!("[state.raw] {}", serde_json::to_string(&st.raw).unwrap_or_default());
                if st.claimable {
                    match adapter.claim(&cred, &st).await {
                        Ok(r) => println!("[claim] status={} amount={:?} message={:?}", r.status, r.amount, r.message),
                        Err(e) => println!("[claim] ERR {e}"),
                    }
                    match adapter.state(&cred).await {
                        Ok(after) => println!("[state-after] checked={} claimable={}", after.checked, after.claimable),
                        Err(e) => println!("[state-after] ERR {e}"),
                    }
                }
            }
            Err(e) => println!("[state] ERR {e}"),
        }
    }
}
