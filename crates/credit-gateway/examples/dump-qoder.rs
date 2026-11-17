use credit_core::db::Store;
fn main() {
    let store = Store::open_default().expect("store");
    for (p, uid, _n, _s, auth) in store.list_accounts().expect("accounts") {
        if p != "qoder" { continue; }
        if let Ok(c) = serde_json::from_str::<credit_core::adapters::Cred>(&auth) {
            println!("{uid}\t{}", c.token);
        }
    }
}
