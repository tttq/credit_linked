use rusqlite::Connection;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).cloned().unwrap_or_default();
    let table = args.get(2).cloned().unwrap_or_default();
    let conn = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).expect("open");
    let tables: Vec<String> = if table.is_empty() {
        conn.prepare("SELECT name FROM sqlite_master WHERE type='table'").unwrap()
            .query_map([], |r| r.get(0)).unwrap().filter_map(|r| r.ok()).collect()
    } else { vec![table.clone()] };
    for t in tables {
        println!("=== {t} ===");
        let mut stmt = match conn.prepare(&format!("SELECT * FROM \"{t}\" ORDER BY rowid DESC LIMIT 3")) { Ok(s) => s, Err(e) => { println!("err: {e}"); continue; } };
        let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let mut rows = stmt.query([]).unwrap();
        while let Ok(Some(row)) = rows.next() {
            for (i, c) in cols.iter().enumerate() {
                let v: String = row.get_ref(i).map(|r| match r {
                    rusqlite::types::ValueRef::Text(b) => String::from_utf8_lossy(b).chars().take(2500).collect(),
                    rusqlite::types::ValueRef::Blob(b) => format!("<blob {}B>", b.len()),
                    rusqlite::types::ValueRef::Integer(i) => i.to_string(),
                    rusqlite::types::ValueRef::Real(f) => f.to_string(),
                    rusqlite::types::ValueRef::Null => "NULL".into(),
                }).unwrap_or_default();
                println!("  {c} = {v}");
            }
            println!("  ---");
        }
    }
}
