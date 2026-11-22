use credit_core::db::Store;
fn b64(s:&str)->Option<Vec<u8>>{
    let mut t = s.replace('-', "+").replace('_', "/");
    while t.len() % 4 != 0 { t.push('='); }
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(t).ok()
}
fn main(){
    let s = Store::open_default().unwrap();
    for (p,u,n,_sec,auth) in s.list_accounts().unwrap() {
        if p != "trae" { continue }
        let c: serde_json::Value = serde_json::from_str(&auth).unwrap_or_default();
        let tok = c.get("token").and_then(|v| v.as_str()).unwrap_or("");
        let parts: Vec<&str> = tok.split('.').collect();
        println!("\n=== {n} uid={u} parts={} ===", parts.len());
        if parts.len() >= 2 {
            if let Some(b) = b64(parts[1]) {
                let txt = String::from_utf8_lossy(&b);
                let v: serde_json::Value = serde_json::from_str(&txt).unwrap_or_default();
                println!("  payload={}", serde_json::to_string(&v).unwrap_or_default());
            }
        }
    }
}
