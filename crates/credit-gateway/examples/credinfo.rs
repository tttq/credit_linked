// 凭据诊断：打印 token 状态并尝试一次 refresh，输出真实错误
use credit_core::adapters::*;
use credit_core::db::Store;
use credit_core::service::adapter_for;

#[tokio::main]
async fn main() {
    let want: Vec<String> = std::env::args().skip(1).collect();
    let s = Store::open_default().unwrap();
    for (p, u, n, _sec, auth) in s.list_accounts().unwrap() {
        if !want.is_empty() && !want.iter().any(|w| *w == p) {
            continue;
        }
        let Ok(c) = serde_json::from_str::<Cred>(&auth) else { continue };
        if u.trim().is_empty() {
            println!("[skip] {p} / {n} uid 为空");
            continue;
        }
        let Some(ad) = adapter_for(&p) else { continue };
        println!("\n=== {p} / {n} (uid={u}) ===");
        println!("  token_len={} refresh_token={} expires_at={:?}",
            c.token.len(),
            c.refresh_token.as_deref().map(|r| r.len()).unwrap_or(0),
            c.expires_at);
        match ad.state(&c).await {
            Ok(st) => println!("  state: checked={} claimable={}", st.checked, st.claimable),
            Err(e) => println!("  state ERR: {e}"),
        }
        match ad.refresh(&c).await {
            Ok(nc) => println!("  refresh OK: 新 token_len={} expires_at={:?}", nc.token.len(), nc.expires_at),
            Err(e) => println!("  refresh ERR: {e}"),
        }
    }
}
