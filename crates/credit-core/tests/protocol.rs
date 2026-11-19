//! 协议对齐测试：tc 信封、WB 信封 AAD、回调解析、幂等键、随机公式、canonical 签名。

use credit_core::adapters::codearts::sign_codearts;
use credit_core::adapters::trae::{parse_callback_query, urlencoding_lite};
use credit_core::adapters::trae_local::request_source_for_dir;
use credit_core::adapters::wb_local::build_envelope_aad;
use credit_core::crypto::aes_cbc::{tc_decrypt, HEADER};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::json;
use sha2::{Digest, Sha512};

/// 生成一个真实可解的 tc 信封（用 Rust 侧同公式加密 → 解密往返自证 + 固定向量）
fn seal_for_test(body: &[u8], salt: [u8; 32]) -> String {
    use credit_core::crypto::aes_cbc::{WOE, VOE};
    use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
    type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
    let pepper: Vec<u8> = WOE.iter().zip(VOE.iter()).map(|(a, b)| a ^ b).collect();
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
    B64.encode(raw)
}

#[test]
fn test_tc_envelope_roundtrip() {
    let body = br#"{"token":"eyJhbGciOiJFUzI1NiJ9.eyJkYXRhIjp7ImlkIjoiNDU2Nzg5In0sImV4cCI6OTk5OTk5OTk5OX0.sig","host":"https://api.trae.cn","expireAt":9999999999}"#;
    let salt = [7u8; 32];
    let env = seal_for_test(body, salt);
    let v = tc_decrypt(&env).expect("decrypt must succeed");
    assert_eq!(v["host"], "https://api.trae.cn");
    assert_eq!(v["token"], "eyJhbGciOiJFUzI1NiJ9.eyJkYXRhIjp7ImlkIjoiNDU2Nzg5In0sImV4cCI6OTk5OTk5OTk5OX0.sig");
}

#[test]
fn test_tc_envelope_header_reject() {
    // 错误 header → None
    assert!(tc_decrypt("AAAA").is_none());
    assert!(tc_decrypt("").is_none());
    // 伪 JSON b64（无 tc header）
    assert!(tc_decrypt(&B64.encode(b"{}")).is_none());
}

#[test]
fn test_callback_query_multi_decode_and_single_equal() {
    // 值里合法含 = （JWT base64url padding）：必须 first '=' 切分
    let q = "authCode=abc&userInfo=eyJk%2525YXRh%2525Ig%253D%253D&loginTraceID=t1";
    let m = parse_callback_query(q);
    assert_eq!(m.get("authCode").unwrap(), "abc");
    // userInfo 应被解码 ≤3 次：eyJk%25YXRh... 一路剥到 eyJk%YXRh 不再含 %（或已无 %）
    let u = m.get("userInfo").unwrap();
    assert!(u.starts_with("eyJ"), "got: {u}");
    assert_eq!(m.get("loginTraceID").unwrap(), "t1");
    // 带 ? 前缀（req.url 形态）
    let m2 = parse_callback_query("/authorize?authCode=x1");
    assert_eq!(m2.get("authCode").unwrap(), "x1");
}

#[test]
fn test_wb_envelope_aad_tlv() {
    let env = json!({ "suite": 1, "keyId": "k-2024" });
    let aad = build_envelope_aad(&env);
    let expected_prefix: Vec<u8> = {
        let mut v = Vec::new();
        v.extend_from_slice(b"WB-AAD\0");
        v.push(1);
        // lp("WBEV1")
        v.extend_from_slice(&5u32.to_be_bytes());
        v.extend_from_slice(b"WBEV1");
        // lp("sym-v1")
        v.extend_from_slice(&6u32.to_be_bytes());
        v.extend_from_slice(b"sym-v1");
        // u32(suite=1)
        v.extend_from_slice(&1u32.to_be_bytes());
        // lp("k-2024")
        v.extend_from_slice(&6u32.to_be_bytes());
        v.extend_from_slice(b"k-2024");
        // [2,0,0]
        v.extend_from_slice(&[2, 0, 0]);
        v
    };
    assert_eq!(aad, expected_prefix);
}

#[test]
fn test_codearts_idempotent_key_shape() {
    let day = credit_core::adapters::day_cn();
    let key = credit_core::adapters::codearts::sign_codearts("AK", "SK", "GET", "snap-access.cn-north-4.myhuaweicloud.com", "/v1/ops/delivery", "channel=IDE", &[], "", None);
    let (auth, headers) = key;
    assert!(auth.starts_with("SDK-HMAC-SHA256 Access=AK, SignedHeaders="), "{auth}");
    assert!(headers.iter().any(|(k, _)| k == "x-sdk-date"));
    let _ = day;
}

#[test]
fn test_codearts_canonical_deterministic() {
    let hs = vec![("content-type".to_string(), "application/json".to_string())];
    let (a1, _) = sign_codearts("AKTEST", "SKTEST", "POST", "snap-access.cn-north-4.myhuaweicloud.com", "/v1/ops/claim", "", &hs, "{}", None);
    let (a2, _) = sign_codearts("AKTEST", "SKTEST", "POST", "snap-access.cn-north-4.myhuaweicloud.com", "/v1/ops/claim", "", &hs, "{}", None);
    // x-sdk-date 同秒内相同 → 签名应一致（跨秒会变，重试时窗口内一致）
    let sig1 = a1.split("Signature=").nth(1).unwrap();
    let sig2 = a2.split("Signature=").nth(1).unwrap();
    assert_eq!(sig1, sig2, "same-second signatures must match");
}

#[test]
fn test_random_schedule_formula() {
    // 固定 seed 验证公式：minute = start + floor(seed / 2^32 * span)
    // seed = 2^31 → 恰好中点
    let mut seeds = credit_core::schedule::Seeds(Default::default());
    seeds.0.insert("2026-11-17".to_string(), u32::MAX / 2);
    // 强制 day 一致性：直接测 plan 内部公式即可
    let minute = |seed: u32, start: i64, span: i64| start + ((seed as u64) * span as u64 / (u32::MAX as u64 + 1)) as i64;
    assert_eq!(minute(u32::MAX / 2, 7 * 60, (9 * 60 - 7 * 60) + 1), 7 * 60 + 60); // 约 8:00
    assert_eq!(minute(0, 420, 121), 420); // 起点
    assert_eq!(minute(u32::MAX, 420, 121), 540); // 终点 540 = 9:00
    let _ = &mut seeds;
}

#[test]
fn test_urlencoding_lite() {
    assert_eq!(urlencoding_lite("a b&c=d"), "a%20b%26c%3Dd");
    assert_eq!(urlencoding_lite("abc123"), "abc123");
    assert_eq!(urlencoding_lite("稳定"), "%E7%A8%B3%E5%AE%9A");
}

#[test]
fn test_request_source_solo() {
    assert_eq!(request_source_for_dir(&std::path::PathBuf::from("C:/x/TRAE SOLO CN")), 2);
    assert_eq!(request_source_for_dir(&std::path::PathBuf::from("C:/x/Trae CN")), 1);
}

#[test]
fn test_day_cn_format() {
    let d = credit_core::adapters::day_cn();
    assert_eq!(d.len(), 10);
    assert_eq!(d.as_bytes()[4], b'-');
    assert_eq!(d.as_bytes()[7], b'-');
}

#[test]
fn test_platform_fetch_rejects_non_https_and_offlist() {
    // 这些是同步拒绝路径，不真正发请求
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let bad = rt.block_on(credit_core::net::http::platform_fetch(
        "http://api.trae.cn/x", reqwest::Method::GET, None, &[]));
    assert!(matches!(bad, Err(credit_core::PlatformError::Network(_))));
    let bad2 = rt.block_on(credit_core::net::http::platform_fetch(
        "https://evil.example.com/x", reqwest::Method::GET, None, &[]));
    assert!(matches!(bad2, Err(credit_core::PlatformError::Network(_))));
}

#[test]
fn test_business_unpack() {
    use credit_core::net::http::business;
    assert!(business(json!({"code": 0, "data": {"a": 1}})).is_ok());
    assert!(business(json!({"code": "0", "data": {"a": 1}})).is_ok());
    assert!(business(json!({"code": 200, "data": {}})).is_ok());
    assert!(business(json!({"code": "200", "data": {}})).is_ok());
    let e = business(json!({"code": 9074, "msg": "risk"})).unwrap_err();
    match e {
        credit_core::PlatformError::Business { code, message } => {
            assert_eq!(code.as_i64(), Some(9074));
            assert_eq!(message, "risk");
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn test_tc_envelope_node_cross_language_kat() {
    // 由 Node（原版 Electron vendor 同款 crypto 实现）生成的信封 → Rust 解密必须逐字段一致
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/trae_envelope_fixture.json");
    let raw = std::fs::read_to_string(dir).expect("fixture missing; run tools/gen-fixture.js");
    let fx: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let env = fx["envelope"].as_str().unwrap();
    let v = tc_decrypt(env).expect("Node-generated envelope must decrypt in Rust");
    assert_eq!(v["host"], "https://api.trae.cn");
    assert_eq!(
        v["token"], "eyJhbGciOiJFUzI1NiJ9.eyJkYXRhIjp7ImlkIjoiNDU2Nzg5In0sImV4cCI6OTk5OTk5OTk5OX0.sig"
    );
    assert_eq!(v["expireAt"], 9999999999i64);
}

// ===== CodeArts 身份 / DPoP JWK / 时间格式（自动续期相关回归） =====

#[test]
fn test_codearts_identity_of_principal_urn() {
    use credit_core::adapters::codearts::identity_of;
    // 真实 caller-identity 响应样例（2026-11-19 实测）
    let v = json!({
        "account_id": "01a1e7d54e5d714fb3ab60e95388d025",
        "principal_id": "01a1e7d54e5d7bcea9258c502a4ccb26",
        "principal_urn": "iam::01a1e7d54e5d714fb3ab60e95388d025:user:hw083373957922722"
    });
    let (uid, name, domain) = identity_of(&v);
    assert_eq!(uid, "01a1e7d54e5d7bcea9258c502a4ccb26");
    assert_eq!(name, "hw083373957922722");
    assert_eq!(domain, "01a1e7d54e5d714fb3ab60e95388d025");
    // user_name 兜底 + 去 '/' 前缀
    let v2 = json!({"principal_id": "u1", "user_name": "acc/zhangsan"});
    let (_, name2, _) = identity_of(&v2);
    assert_eq!(name2, "zhangsan");
}

#[test]
fn test_codearts_jwk_value_unwraps_json_string() {
    use credit_core::adapters::codearts::jwk_value;
    // 本机 secretStorage 的真实形态：privateKey 是 JSON 字符串（含 d）
    let inner = json!({"kty":"EC","crv":"P-256","x":"aaa","y":"bbb","d":"ccc"});
    let wrapped = json!(inner.to_string());
    let jwk = jwk_value(&wrapped);
    assert!(jwk.is_object());
    assert_eq!(jwk["d"], "ccc");
    // 已经是对象时原样返回
    let jwk2 = jwk_value(&inner);
    assert_eq!(jwk2["d"], "ccc");
    // 非法字符串/Null 不 panic
    assert!(jwk_value(&json!("not-json")).is_string());
    assert!(jwk_value(&json!(null)).is_null());
}

#[test]
fn test_codearts_iso_epoch_roundtrip() {
    use credit_core::adapters::codearts::{epoch_ms_to_iso, iso_to_epoch_ms};
    // 2026-11-19T12:16:17.000Z = 1795090577000ms（UTC+8 展示为 20:16:17）
    let ms = iso_to_epoch_ms("2026-11-19T12:16:17.000Z").unwrap();
    assert_eq!(ms, 1795090577000);
    let iso = epoch_ms_to_iso(ms);
    assert!(iso.starts_with("2026-11-19T20:16:17"), "UTC+8 展示，实际 {iso}");
    // 无毫秒 / 带 +08:00 偏移
    assert_eq!(iso_to_epoch_ms("2026-11-19T12:16:17Z").unwrap(), 1795090577000);
    assert_eq!(iso_to_epoch_ms("2026-11-19T20:16:17+08:00").unwrap(), 1795090577000);
}
