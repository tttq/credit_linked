//! platformFetch 等价实现：https 强制 + 17 host 白名单 + redirect none + 15s 超时
//! + 状态码映射 + 强制 JSON + business 解包。行为逐条对齐 adapters/http.ts。

use std::sync::OnceLock;
use std::time::Duration;

use reqwest::{Client, Method, Url};
use serde_json::Value;

use crate::error::PlatformError;

pub const HOSTS: [&str; 17] = [
    "zcode.z.ai",
    "bigmodel.cn",
    "api.trae.cn",
    "api.trae.com.cn",
    "www.trae.cn",
    "www.codebuddy.cn",
    "www.workbuddy.cn",
    "www.workbuddy.ai",
    "www.codebuddy.ai",
    "workbuddy.cc",
    "copilot.tencent.com",
    "openapi.qoder.com.cn",
    "openapi.qoder.sh",
    "agent.minimax.cn",
    "account.minimax.cn",
    "snap-access.cn-north-4.myhuaweicloud.com",
    "sts.cn-north-4.myhuaweicloud.com",
];

/// 白名单外但登录流程必须可用的主机（与原版 login.ts 一致：授权页仅限浏览器打开，
/// 轮询接口都在白名单内；此处仅放行 Trae/Qoder 换票等已验证端点）。
pub fn host_allowed(host: &str) -> bool {
    HOSTS.contains(&host)
}

fn client() -> &'static Client {
    static C: OnceLock<Client> = OnceLock::new();
    C.get_or_init(|| {
        Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(15))
            .user_agent("credit-rs/0.1.77")
            .build()
            .expect("reqwest client")
    })
}

pub async fn platform_fetch(
    url: &str,
    method: Method,
    body: Option<Value>,
    headers: &[(&str, String)],
) -> Result<Value, PlatformError> {
    let u = Url::parse(url).map_err(|e| PlatformError::Network(e.to_string()))?;
    let host = u.host_str().unwrap_or("").to_string();
    if u.scheme() != "https" || !host_allowed(&host) {
        return Err(PlatformError::Network(format!(
            "host not allowed: {host} (url={url})"
        )));
    }
    let mut req = client().request(method, u).timeout(Duration::from_secs(15));
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    if let Some(b) = &body {
        req = req.json(b);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| PlatformError::Network(e.to_string()))?;
    match resp.status().as_u16() {
        401 => return Err(PlatformError::Auth),
        429 => return Err(PlatformError::Rate),
        403 => return Err(PlatformError::Forbidden),
        _ => {}
    }
    resp.json::<Value>()
        .await
        .map_err(|e| PlatformError::Format(e.to_string()))
}

/// business(body)：code 缺省视为成功；code ∈ {0,"0",200,"200"} 放行；否则 BUSINESS（原码原文）。
/// scope：data 为对象/数组时解包，否则返回整个 body（vendor scope(body) 语义）。
pub fn business(v: Value) -> Result<Value, PlatformError> {
    let bad = match v.get("code") {
        None | Some(Value::Null) => false,
        Some(Value::Number(n)) => !(n.as_i64() == Some(0) || n.as_i64() == Some(200)),
        Some(Value::String(s)) => !(s == "0" || s == "200"),
        Some(_) => true,
    };
    if bad {
        return Err(PlatformError::business_of(&v));
    }
    match v.get("data") {
        Some(x) if x.is_object() || x.is_array() => Ok(x.clone()),
        _ => Ok(v),
    }
}
