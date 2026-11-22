// Trae 登录字段校验探针：判断 10101「无效参数」到底由哪个字段引发
// 用法：cargo run -p credit-gateway --example traeprobe
use credit_core::adapters::trae;

#[tokio::main]
async fn main() {
    let l = trae::start_login();
    let si = trae::system_information();
    let dev = trae::device_info(&l.identity, trae::APP_VERSION, &si);
    println!("=== SysInfo ===\nmodel={:?}\nmanufacturer={:?}\ncpu={:?}\nos={:?}\nver={:?}\n",
        si.device_model, si.device_manufacturer, si.cpu_brand, si.os_name, si.os_version);
    println!("=== DeviceInfo ===\n{}\n", serde_json::to_string_pretty(&dev).unwrap_or_default());
    let cli = reqwest::Client::builder().build().unwrap();
    let url = format!("{}{}", trae::API_BASE_CN, trae::EXCHANGE_PATH);

    let variants: Vec<(&str, serde_json::Value)> = vec![
        ("full+code", serde_json::json!({
            "ClientID": trae::CLIENT_ID_CN, "AuthCode": "INVALID_PROBE_CODE",
            "CodeVerifier": l.verifier, "DeviceInfo": dev.clone(), "IDEVersion": trae::APP_VERSION })),
        ("no-DeviceInfo", serde_json::json!({
            "ClientID": trae::CLIENT_ID_CN, "AuthCode": "INVALID_PROBE_CODE",
            "CodeVerifier": l.verifier, "IDEVersion": trae::APP_VERSION })),
        ("empty-DeviceInfo", serde_json::json!({
            "ClientID": trae::CLIENT_ID_CN, "AuthCode": "INVALID_PROBE_CODE",
            "CodeVerifier": l.verifier, "DeviceInfo": {}, "IDEVersion": trae::APP_VERSION })),
        ("no-CodeVerifier", serde_json::json!({
            "ClientID": trae::CLIENT_ID_CN, "AuthCode": "INVALID_PROBE_CODE",
            "DeviceInfo": dev.clone(), "IDEVersion": trae::APP_VERSION })),
        ("no-ClientID", serde_json::json!({
            "AuthCode": "INVALID_PROBE_CODE", "CodeVerifier": l.verifier,
            "DeviceInfo": dev.clone(), "IDEVersion": trae::APP_VERSION })),
        ("bad-ClientID", serde_json::json!({
            "ClientID": "zzzzzzzzzzzzzz", "AuthCode": "INVALID_PROBE_CODE",
            "CodeVerifier": l.verifier, "DeviceInfo": dev.clone(), "IDEVersion": trae::APP_VERSION })),
        ("only-Code", serde_json::json!({
            "ClientID": trae::CLIENT_ID_CN, "AuthCode": "INVALID_PROBE_CODE" })),
    ];
    for (tag, body) in variants {
        match cli.post(&url)
            .header("Content-Type", "application/json")
            .header("x-cloudide-token", "")
            .json(&body).send().await
        {
            Ok(r) => {
                let st = r.status();
                let t = r.text().await.unwrap_or_default();
                let code = t.split("\"Code\":\"").nth(1).and_then(|s| s.split('"').next()).unwrap_or("-");
                println!("[{tag:18}] HTTP {st} code={code}");
                if code == "-" || code.is_empty() {
                    println!("    {}", &t[..t.len().min(300)]);
                }
            }
            Err(e) => println!("[{tag:18}] ERR {e}"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
}
