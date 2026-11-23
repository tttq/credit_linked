//! credit-cli：协议对齐验证宿主。
//! 子命令：
//!   import trae                       — 本机扫描 Trae 登录态并入库
//!   import wb [--intl]                — 本机扫描 WorkBuddy 会话并入库
//!   import qoder [--intl]             — 本机扫描 Qoder
//!   state <provider> <uid>            — 查询签到状态
//!   checkin <provider> <uid>          — 执行签到
//!   quota <provider> <uid>            — 查询额度
//!   accounts                          — 列出账号
//!   selftest                          — 跑协议自检（解密往返/签名/公式）

use clap::{Parser, Subcommand};
use credit_core::adapters::trae_local;
use credit_core::db::Store;
use credit_core::service::{adapter_for, Service};
use std::sync::Arc;

/// 命令行参数定义
#[derive(Parser)]
#[command(name = "credit-cli", version = "0.1.77")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

/// 子命令集合
#[derive(Subcommand)]
enum Cmd {
    /// 本机扫描导入平台账号（platform: trae | wb | qoder；--intl 国际版）
    Import {
        platform: String,
        #[arg(long)]
        intl: bool,
    },
    /// 查询账号签到状态
    State { provider: String, uid: String },
    /// 执行签到
    Checkin { provider: String, uid: String },
    /// 查询额度
    Quota { provider: String, uid: String },
    /// 列出全部账号
    Accounts,
    /// 协议自检（tc 信封加密/解密往返）
    Selftest,
}

/// 打开默认数据库（每条命令独立连接）
fn store() -> anyhow::Result<Arc<Store>> {
    Ok(Arc::new(Store::open_default()?))
}

/// 凭据序列化入库（upsert）并打印结果
fn save_cred(s: &Store, cred: &credit_core::adapters::Cred) -> anyhow::Result<()> {
    let auth = serde_json::to_string(cred)?;
    s.upsert_account(&cred.provider, &cred.uid, &cred.name, "", &auth)?;
    println!("saved {}:{} ({})", cred.provider, cred.uid, cred.name);
    Ok(())
}

/// CLI 入口：解析子命令并分发执行
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Import { platform, intl } => {
            let s = store()?;
            match platform.as_str() {
                "trae" => {
                    let Some(live) = trae_local::read_live_auth() else {
                        anyhow::bail!("未发现可读取的当前登录信息（Trae 未登录或未安装）");
                    };
                    if live.is_intl {
                        anyhow::bail!("检测到国际版登录态（host={}），当前版本仅支持国内版", live.host.unwrap_or_default());
                    }
                    let token = live.auth.get("token").and_then(|t| t.as_str()).unwrap_or_default().to_string();
                    let uid = jwt_uid(&token);
                    let cred = credit_core::adapters::Cred {
                        provider: "trae".into(),
                        uid,
                        name: format!("Trae ({})", live.data_dir.file_name().unwrap_or_default().to_string_lossy()),
                        token,
                        refresh_token: live.auth.get("refreshToken").and_then(|t| t.as_str()).map(|s| s.to_string()),
                        expires_at: None,
                        meta: serde_json::json!({
                            "deviceId": live.device_id,
                            "host": live.host.unwrap_or_else(|| "https://api.trae.cn".into()),
                            "reqSource": trae_local::request_source_for_dir(&live.data_dir),
                        }),
                    };
                    save_cred(&s, &cred)?;
                }
                "wb" | "workbuddy" => {
                    let sess = credit_core::adapters::wb_local::read_session(intl, None)
                        .ok()
                        .flatten();
                    let Some(sess) = sess else { anyhow::bail!("未发现 WorkBuddy 会话文件（未登录或未安装）") };
                    let Some(acc) = credit_core::adapters::wb_local::session_to_account(&sess) else {
                        anyhow::bail!("会话文件缺少 accessToken");
                    };
                    let cred = credit_core::adapters::Cred {
                        provider: if intl { "workbuddy-intl".into() } else { "workbuddy".into() },
                        uid: acc.get("uid").and_then(|u| u.as_str()).unwrap_or_default().to_string(),
                        name: acc.get("name").and_then(|n| n.as_str()).unwrap_or("WorkBuddy").to_string(),
                        token: acc.get("token").and_then(|t| t.as_str()).unwrap_or_default().to_string(),
                        refresh_token: acc.get("refreshToken").and_then(|t| t.as_str()).map(|s| s.to_string()),
                        expires_at: None,
                        meta: serde_json::json!({
                            "domain": acc.get("domain").and_then(|d| d.as_str()).unwrap_or(if intl { "www.workbuddy.ai" } else { "www.codebuddy.cn" }),
                            "enterpriseId": acc.get("enterpriseId").and_then(|e| e.as_str()).unwrap_or_default(),
                        }),
                    };
                    save_cred(&s, &cred)?;
                }
                "qoder" => {
                    let Some(v) = credit_core::adapters::qoder_local::read_auth(intl)? else {
                        anyhow::bail!("未发现 Qoder auth.v1.dat（未登录或未安装）");
                    };
                    let cred = credit_core::adapters::Cred {
                        provider: if intl { "qoder-intl".into() } else { "qoder".into() },
                        uid: v.pointer("/user/id").and_then(|u| u.to_string().trim_matches('"').to_string().into()).unwrap_or_default(),
                        name: v.pointer("/user/name").and_then(|n| n.as_str()).unwrap_or("Qoder").to_string(),
                        token: v.get("token").and_then(|t| t.as_str()).unwrap_or_default().to_string(),
                        refresh_token: None,
                        expires_at: v.get("expiresAt").and_then(|e| e.as_str()).map(|s| s.to_string()),
                        meta: serde_json::json!({}),
                    };
                    save_cred(&s, &cred)?;
                }
                "minimax" => {
                    // 走 Service::import_account 的真实导入路径（含 profile uid/name 校正与多记录导入），
                    // 保证 CLI 与 GUI「扫描本机」完全同一条链路。
                    let svc = Service { store: s.clone() };
                    let v = svc.import_account("minimax", intl).await?;
                    println!("saved minimax:{} ({})", v.get("uid").and_then(|u| u.as_str()).unwrap_or(""), v.get("name").and_then(|n| n.as_str()).unwrap_or(""));
                }
                other => anyhow::bail!("platform {other} 不支持 CLI 导入（doubao 需窗口驱动，codearts 走登录流程）"),
            }
            Ok(())
        }
        Cmd::State { provider, uid } => {
            let svc = Service { store: store()? };
            let a = adapter_for(&provider).ok_or_else(|| anyhow::anyhow!("no adapter for {provider}"))?;
            let cred = load_cred(&svc, &provider, &uid)?;
            let st = a.state(&cred).await?;
            println!("{}", serde_json::to_string_pretty(&st)?);
            Ok(())
        }
        Cmd::Checkin { provider, uid } => {
            let svc = Service { store: store()? };
            let out = svc.checkin_account(&provider, &uid).await?;
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "provider": out.provider, "uid": out.uid, "status": out.status,
                "amount": out.amount, "message": out.message,
            }))?);
            Ok(())
        }
        Cmd::Quota { provider, uid } => {
            let svc = Service { store: store()? };
            let q = svc.quota_account(&provider, &uid).await?;
            println!("{}", serde_json::to_string_pretty(&q)?);
            Ok(())
        }
        Cmd::Accounts => {
            let s = store()?;
            for (p, u, n, _, _) in s.list_accounts()? {
                println!("{p}\t{u}\t{n}");
            }
            Ok(())
        }
        Cmd::Selftest => {
            println!("selftest: run 'cargo test -p credit-core' for full protocol suite");
            // 快速自检：tc 往返
            let body = br#"{"token":"t","host":"https://api.trae.cn"}"#;
            let salt = [1u8; 32];
            let env = seal_for_test(body, salt);
            let v = credit_core::crypto::aes_cbc::tc_decrypt(&env);
            println!("tc envelope roundtrip: {}", v.is_some());
            Ok(())
        }
    }
}

/// 按 provider+uid 从 accounts 表加载凭据
fn load_cred(svc: &Service, provider: &str, uid: &str) -> anyhow::Result<credit_core::adapters::Cred> {
    let accounts = svc.store.list_accounts()?;
    let Some((_, _, _, _, auth)) = accounts.iter().find(|(p, u, _, _, _)| p == provider && u == uid) else {
        anyhow::bail!("account {provider}:{uid} not found");
    };
    Ok(serde_json::from_str(auth)?)
}

/// 解 Trae JWT payload 取 uid（data.id；解析失败返回空串）
fn jwt_uid(token: &str) -> String {
    use base64::Engine;
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 { return String::new(); }
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[1]) else { return String::new() };
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
    v.pointer("/data/id").and_then(|i| i.as_str()).unwrap_or("").to_string()
}

/// 按 tc 信封格式加密（selftest 解密往返用；密钥派生对齐 credit-core）
fn seal_for_test(body: &[u8], salt: [u8; 32]) -> String {
    use credit_core::crypto::aes_cbc::{HEADER, VOE, WOE};
    use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
    type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
    use base64::Engine;
    let pepper: Vec<u8> = WOE.iter().zip(VOE.iter()).map(|(a, b)| a ^ b).collect();
    use sha2::{Digest, Sha512};
    let h1 = Sha512::digest(salt);
    let mut cat = Vec::new();
    cat.extend_from_slice(&h1);
    cat.extend_from_slice(&pepper);
    let k = Sha512::digest(&cat);
    let mut mac_and_body = Sha512::digest(body).to_vec();
    mac_and_body.extend_from_slice(body);
    let enc = Aes128CbcEnc::new((&k[..16]).into(), (&k[16..32]).into());
    let mut out = vec![0u8; mac_and_body.len() + 16];
    out[..mac_and_body.len()].copy_from_slice(&mac_and_body);
    let n = enc.encrypt_padded_mut::<Pkcs7>(&mut out, mac_and_body.len()).unwrap().len();
    let mut raw = Vec::new();
    raw.extend_from_slice(&HEADER);
    raw.extend_from_slice(&salt);
    raw.extend_from_slice(&out[..n]);
    base64::engine::general_purpose::STANDARD.encode(raw)
}
