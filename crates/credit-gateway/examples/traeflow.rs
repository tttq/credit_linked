// Trae 登录全链路闭环探针（不需要真人授权）：
// 1) start_login 起本机回调服务并拿到授权 URL
// 2) 用假 authCodeInfo 模拟浏览器回调（官方真实回调形态）
// 3) poll_login 观察状态流转：pending → error（换码失败但链路已通）
// 若状态能变成 error 且 error 文本是换码失败（而不是「缺少 AuthCode」），
// 说明回调字段解析 + 轮询状态机已修好。
use credit_core::login;

#[tokio::main]
async fn main() {
    let (sid, url, exp) = match login::start_login("trae").await {
        Ok(v) => v,
        Err(e) => { println!("start_login 失败: {e}"); return; }
    };
    println!("sessionId={sid}\nexpiresIn={exp}s\nurl={url}\n");

    let port = url.split("auth_callback_url=").nth(1)
        .and_then(|s| s.split('%').next())
        .unwrap_or("");
    let _ = port;

    // 从 URL 里取回调端口（auth_callback_url 被编码成 http%3A%2F%2F127.0.0.1%3A3379%2Fauthorize）
    let cb_port = url.split("auth_callback_url=http%3A%2F%2F127.0.0.1%3A").nth(1)
        .and_then(|s| s.split("%2F").next())
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(3379);
    println!("回调端口 = {cb_port}");

    // 官方回调形态：authCodeInfo=<JSON>，userInfo=<JSON>
    let enc = credit_core::adapters::trae::urlencoding_lite;
    let auth_code_info = enc(r#"{"AuthCode":"FAKE_CODE_FOR_PIPELINE_TEST"}"#);
    let user_info = enc(r#"{"UserID":"999999999999999","ScreenName":"探针账号"}"#);
    let cb_url = format!(
        "http://127.0.0.1:{cb_port}/authorize?authCodeInfo={auth_code_info}&userInfo={user_info}&scope=TRAE&loginScope=toc"
    );
    match reqwest::get(&cb_url).await {
        Ok(r) => println!("回调注入 HTTP {}", r.status()),
        Err(e) => println!("回调注入失败: {e}"),
    }

    for i in 0..10 {
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        match login::poll_login(&sid).await {
            Ok(o) => {
                println!("[{i}] status={} final={} error={:?} account={:?}",
                    o.status, o.is_final(), o.error,
                    o.credential.as_ref().map(|c| (c.uid.clone(), c.name.clone())));
                if o.is_final() { break; }
            }
            Err(e) => { println!("[{i}] poll err = {e}"); break; }
        }
    }
}
