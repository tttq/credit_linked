//! AES-256-GCM 统一封装（workbuddy 信封 / qoder v10 / codearts v10 共用）。
//! qoder/codearts blob 布局："v10" || nonce[12] || ct || tag[16]，nonce 取 [3:15]。

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};

/// AES-256-GCM 解密（ct_with_tag = 密文 + 16B 认证标签，可带 AAD；失败返回错误说明）。
pub fn gcm_decrypt(key: &[u8], nonce: &[u8], ct_with_tag: &[u8], aad: &[u8]) -> Result<Vec<u8>, String> {
    if key.len() != 32 {
        return Err("key must be 32B".into());
    }
    if nonce.len() != 12 {
        return Err("nonce must be 12B".into());
    }
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    cipher
        .decrypt(Nonce::from_slice(nonce), Payload { msg: ct_with_tag, aad })
        .map_err(|e| format!("gcm: {e}"))
}

/// AES-256-GCM 加密（输出 = 密文 + 16B 认证标签）。
pub fn gcm_seal(key: &[u8], nonce: &[u8], pt: &[u8], aad: &[u8]) -> Result<Vec<u8>, String> {
    if key.len() != 32 || nonce.len() != 12 {
        return Err("bad params".into());
    }
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    cipher
        .encrypt(Nonce::from_slice(nonce), Payload { msg: pt, aad })
        .map_err(|e| format!("gcm: {e}"))
}

/// "v10" 前缀 blob（Chromium os_crypt 风格）：剥离前缀后 nonce=[0:12], ct+tag=其余
pub fn decrypt_v10_blob(blob: &[u8], key: &[u8], aad: &[u8]) -> Result<Vec<u8>, String> {
    if blob.len() < 3 + 12 + 16 || &blob[..3] != b"v10" {
        return Err("not a v10 blob".into());
    }
    let nonce = &blob[3..15];
    gcm_decrypt(key, nonce, &blob[15..], aad)
}
