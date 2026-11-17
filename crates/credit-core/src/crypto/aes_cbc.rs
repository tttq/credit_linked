//! Trae「tc」信封解密（vendor/traeLocal.mjs 逐字节对齐）。
//! raw[0:6]==HEADER; salt=raw[6:38]; key=SHA512(SHA512(salt)||pepper) 前32B;
//! AES-128-CBC/PKCS7 解 raw[38:]; 明文前64B==SHA512(体) 完整性校验。

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha512};

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

pub const HEADER: [u8; 6] = [116, 99, 5, 16, 0, 0];

pub const WOE: [u8; 64] = [
    82, 9, 106, 213, 48, 54, 165, 56, 191, 64, 163, 158, 129, 243, 215, 251, 124, 227, 57, 130, 155,
    47, 255, 135, 52, 142, 67, 68, 196, 222, 233, 203, 84, 123, 148, 50, 166, 194, 35, 61, 238, 76,
    149, 11, 66, 250, 195, 78, 8, 46, 161, 102, 40, 217, 36, 178, 118, 91, 162, 73, 109, 139, 209, 37,
];
pub const VOE: [u8; 64] = [
    31, 221, 168, 51, 136, 7, 199, 49, 177, 18, 16, 89, 39, 128, 236, 95, 96, 81, 127, 169, 25, 181,
    74, 13, 45, 229, 122, 159, 147, 201, 156, 239, 160, 224, 59, 77, 174, 42, 245, 176, 200, 235,
    187, 60, 131, 83, 153, 97, 23, 43, 4, 126, 186, 119, 214, 38, 225, 105, 20, 99, 85, 33, 12, 125,
];
pub const JOE: [u8; 64] = [
    191, 192, 216, 250, 122, 246, 220, 97, 31, 254, 98, 27, 8, 72, 71, 176, 135, 99, 96, 18, 127,
    101, 203, 104, 211, 102, 191, 125, 37, 72, 150, 156, 51, 229, 121, 35, 17, 153, 141, 177, 110,
    131, 150, 128, 172, 255, 254, 6, 18, 140, 55, 62, 236, 249, 135, 64, 135, 12, 117, 4, 89, 149,
    168, 209,
];
pub const HOE: [u8; 64] = [
    246, 204, 26, 232, 232, 70, 129, 109, 223, 146, 169, 242, 23, 241, 105, 145, 50, 196, 165, 42,
    254, 120, 3, 54, 244, 207, 209, 85, 53, 6, 138, 106, 175, 148, 31, 204, 186, 186, 165, 182, 87,
    142, 49, 10, 39, 110, 26, 154, 86, 56, 173, 125, 18, 64, 198, 225, 99, 99, 83, 82, 191, 134, 76,
    170,
];

fn xor(a: &[u8; 64], b: &[u8; 64]) -> [u8; 64] {
    let mut out = [0u8; 64];
    for i in 0..64 {
        out[i] = a[i] ^ b[i];
    }
    out
}

/// 解密 tc 信封 base64 → JSON。解不开返回 None（由调用方降级，与原版一致）。
pub fn tc_decrypt(b64: &str) -> Option<Value> {
    if b64.trim().is_empty() {
        return None;
    }
    let raw = B64.decode(b64.trim()).ok()?;
    if raw.len() < 54 || raw[..6] != HEADER {
        return None;
    }
    let salt = &raw[6..38];
    let cipher = &raw[38..];
    for pepper in [xor(&WOE, &VOE), xor(&JOE, &HOE)] {
        let h1 = Sha512::digest(salt);
        let mut cat = Vec::with_capacity(64 + 64);
        cat.extend_from_slice(&h1);
        cat.extend_from_slice(&pepper);
        let k = Sha512::digest(&cat);
        let dec = Aes128CbcDec::new((&k[..16]).into(), (&k[16..32]).into());
        let mut buf = cipher.to_vec();
        let plain = match dec.decrypt_padded_mut::<Pkcs7>(&mut buf) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if plain.len() <= 64 {
            continue;
        }
        let (mac, body) = plain.split_at(64);
        if Sha512::digest(body).as_slice() != mac {
            continue;
        }
        if let Ok(v) = serde_json::from_slice::<Value>(body) {
            return Some(v);
        }
    }
    None
}

/// 对应 sealEncrypt（写回 storage.json 用；本轮只读场景未启用，保留对齐）
pub fn tc_encrypt(plain_body: &[u8]) -> Option<String> {
    use aes::cipher::{BlockEncryptMut, KeyIvInit};
    type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
    use cbc::cipher::block_padding::Pkcs7 as Pad;
    let mut salt = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut salt);
    let pepper = xor(&WOE, &VOE);
    let h1 = Sha512::digest(salt);
    let mut cat = Vec::with_capacity(128);
    cat.extend_from_slice(&h1);
    cat.extend_from_slice(&pepper);
    let k = Sha512::digest(&cat);
    let mut mac_and_body = Sha512::digest(plain_body).to_vec();
    mac_and_body.extend_from_slice(plain_body);
    let enc = Aes128CbcEnc::new((&k[..16]).into(), (&k[16..32]).into());
    let mut out = vec![0u8; mac_and_body.len() + 16];
    out[..mac_and_body.len()].copy_from_slice(&mac_and_body);
    let n = enc.encrypt_padded_mut::<Pad>(&mut out, mac_and_body.len()).ok()?.len();
    let mut raw = Vec::with_capacity(38 + n);
    raw.extend_from_slice(&HEADER);
    raw.extend_from_slice(&salt);
    raw.extend_from_slice(&out[..n]);
    Some(B64.encode(raw))
}
