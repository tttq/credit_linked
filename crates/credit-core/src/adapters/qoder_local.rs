//! Qoder 本机凭据（auth.v1.dat "v10" GCM + Local State DPAPI）。

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::Value;

use crate::crypto::aes_gcm::decrypt_v10_blob;

pub fn app_dir(intl: bool) -> Option<std::path::PathBuf> {
    let base = crate::platform_dirs::electron_userdata_root();
    let candidates: &[&str] = if intl {
        &["com.qoder.app.stable", "com.qoder.app", "Qoder", "qoder"]
    } else {
        &["com.qodercn.app.stable", "com.qodercn.app", "Qoder CN", "qoder-cn"]
    };
    crate::platform_dirs::find_app_dir(&base, candidates)
}

/// Local State os_crypt.encrypted_key（"DPAPI" 前缀 5B）→ DPAPI → AES-256 key
pub fn load_aes_key(app: &std::path::Path) -> anyhow::Result<[u8; 32]> {
    let ls: Value = serde_json::from_reader(std::fs::File::open(app.join("Local State"))?)?;
    let enc = ls["os_crypt"]["encrypted_key"].as_str().ok_or_else(|| anyhow::anyhow!("no encrypted_key"))?;
    let raw = B64.decode(enc)?;
    if raw.len() < 5 || &raw[..5] != b"DPAPI" {
        anyhow::bail!("not DPAPI prefixed");
    }
    let key = crate::vault::dpapi_unprotect(&raw[5..])?;
    let mut out = [0u8; 32];
    out.copy_from_slice(&key);
    Ok(out)
}

pub fn read_auth(intl: bool) -> anyhow::Result<Option<Value>> {
    let Some(app) = app_dir(intl) else { return Ok(None) };
    let dat = app.join("auth.v1.dat");
    if !dat.exists() {
        return Ok(None);
    }
    let blob = std::fs::read(&dat)?;
    let key = load_aes_key(&app)?;
    let pt = decrypt_v10_blob(&blob, &key, b"").map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(Some(serde_json::from_slice(&pt)?))
}
