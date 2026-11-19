//! safeStorage(DPAPI) 等价：Windows 用 CryptProtectData/CryptUnprotectData + base64；
//! 非 Windows 平台（macOS/Linux CI 构建）提供 base64 占位实现，保证可编译可运行。

/// DPAPI 加密原始字节（当前用户作用域；结果仅本机当前用户可解）。
#[cfg(windows)]
pub fn dpapi_protect(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    use anyhow::anyhow;
    use windows::Win32::Foundation::LocalFree;
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    use windows::core::PCWSTR;
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut out = CRYPT_INTEGER_BLOB::default();
        CryptProtectData(
            &input,
            PCWSTR::null(),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
        .map_err(|e| anyhow!("CryptProtectData: {e}"))?;
        let slice = std::slice::from_raw_parts(out.pbData, out.cbData as usize);
        let v = slice.to_vec();
        let _ = LocalFree(windows::Win32::Foundation::HLOCAL(out.pbData as *mut _));
        Ok(v)
    }
}

/// DPAPI 解密原始字节（dpapi_protect 的逆操作）。
#[cfg(windows)]
pub fn dpapi_unprotect(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    use anyhow::anyhow;
    use windows::Win32::Foundation::LocalFree;
    use windows::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPT_INTEGER_BLOB,
    };
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut out = CRYPT_INTEGER_BLOB::default();
        CryptUnprotectData(&input, None, None, None, None, 0, &mut out)
            .map_err(|e| anyhow!("CryptUnprotectData: {e}"))?;
        let slice = std::slice::from_raw_parts(out.pbData, out.cbData as usize);
        let v = slice.to_vec();
        let _ = LocalFree(windows::Win32::Foundation::HLOCAL(out.pbData as *mut _));
        Ok(v)
    }
}

/// 非 Windows：尝试写入系统 keyring；失败静默降级为原样返回，保证功能可用。
#[cfg(not(windows))]
pub fn dpapi_protect(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    // macOS Keychain / Linux libsecret 通过 keyring crate；失败时退回明文（保证功能可用）
    use keyring::Entry;
    let svc = "credit-rs";
    let user = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_else(|_| "default".into());
    match Entry::new(svc, &user).and_then(|e| e.set_password(&String::from_utf8_lossy(data))) {
        Ok(_) => Ok(data.to_vec()), // keyring 存的是同一份；返回值仍带在 DB 里（对齐 Windows 语义）
        Err(_) => Ok(data.to_vec()),
    }
}

/// 非 Windows：原样返回（与降级语义配对，DB 内值即明文）。
#[cfg(not(windows))]
pub fn dpapi_unprotect(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    // keyring 里的值与 DB 内一致；直接返回（不使用系统加密存储作唯一来源，避免跨机迁移失效）
    Ok(data.to_vec())
}

/// 加密并 base64 编码为字符串（入库存 secret 列用）。
pub fn seal_b64(plain: &str) -> anyhow::Result<String> {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    Ok(B64.encode(dpapi_protect(plain.as_bytes())?))
}

/// seal_b64 的逆操作：base64 解码后解密还原明文。
pub fn open_b64(b64: &str) -> anyhow::Result<String> {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    let raw = B64.decode(b64)?;
    Ok(String::from_utf8(dpapi_unprotect(&raw)?)?)
}
