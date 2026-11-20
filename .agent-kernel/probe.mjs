import fs from 'node:fs';
const t = fs.readFileSync('E:/auto_change/credit-rs/crates/credit-gateway/src/server.rs', 'utf8');
const i = t.indexOf('let log_id = st.store.log_gateway_request(');
console.log(JSON.stringify(t.slice(i, i + 200)));
