// Trae 多账号并行登录探针（不需要真人授权）：
// 先并发开两个登录会话（各自独立的回调端口），再分别注入官方形态回调并轮询。
// 期望：两个会话端口不同、回调都被正确识别（错误文本是「换码失败」而不是
// 「缺少 AuthCode」）、互不影响 —— 证明两个账号可以并行登录导入。
use credit_core::login;

fn cb_port(url: &str) -> u16 {
    url.split("auth_callback_url=http%3A%2F%2F127.0.0.1%3A")
        .nth(1)
        .and_then(|s| s.split("%2F").next())
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0)
}

#[tokio::main]
async fn main() {
    let enc = credit_core::adapters::trae::urlencoding_lite;
    let mut sids: Vec<(String, u16)> = Vec::new();
    // ① 先全部发起（并行场景：两个会话同时处于待授权状态）
    for i in 0..2 {
        match login::start_login("trae").await {
            Ok((sid, url, _e)) => {
                let p = cb_port(&url);
                println!("[{i}] sid={sid} port={p}");
                sids.push((sid, p));
            }
            Err(e) => {
                println!("[{i}] start_login 失败: {e}");
                return;
            }
        }
    }
    // ② 再分别注入回调
    for (i, (sid, port)) in sids.clone().into_iter().enumerate() {
        let ai = enc(r#"{"AuthCode":"FAKE_CODE_MULTI"}"#);
        let ui = enc(&format!(
            r#"{{"UserID":"99999999999999{i}","ScreenName":"探针账号{i}"}}"#
        ));
        let cb = format!(
            "http://127.0.0.1:{port}/authorize?authCodeInfo={ai}&userInfo={ui}&scope=TRAE&loginScope=toc"
        );
        let r = reqwest::get(&cb).await;
        println!("[{i}] sid={sid:.8} 回调注入 {:?}", r.map(|x| x.status().as_u16()));
    }

    for round in 0..12 {
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        let mut finals = 0;
        for (i, (sid, _p)) in sids.iter().enumerate() {
            match login::poll_login(sid).await {
                Ok(o) => {
                    let brief: String = o
                        .error
                        .as_deref()
                        .unwrap_or("")
                        .chars()
                        .take(70)
                        .collect();
                    println!("[r{round}][{i}] status={} final={} err={brief}", o.status, o.is_final());
                    if o.is_final() {
                        finals += 1;
                    }
                }
                Err(e) => println!("[r{round}][{i}] poll err = {e}"),
            }
        }
        if finals >= sids.len() {
            break;
        }
    }
    let ports: Vec<u16> = sids.iter().map(|(_, p)| *p).collect();
    println!("sessions={} ports={ports:?} distinct={}", sids.len(), ports[0] != ports[1]);
}
