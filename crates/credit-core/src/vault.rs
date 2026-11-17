//! safeStorage(DPAPI) 等价：Windows 用 CryptProtectData/CryptUnprotectData + base64；
//! 非 Windows 平台（macOS/Linux CI 构建）提供 base64 占位实现，保证可编译可运行。

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

#[cfg(not(windows))]
pub fn dpapi_protect(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    Ok(data.to_vec())
}

#[cfg(not(windows))]
pub fn dpapi_unprotect(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    Ok(data.to_vec())
}

pub fn seal_b64(plain: &str) -> anyhow::Result<String> {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    Ok(B64.encode(dpapi_protect(plain.as_bytes())?))
}

pub fn open_b64(b64: &str) -> anyhow::Result<String> {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    let raw = B64.decode(b64)?;
    Ok(String::from_utf8(dpapi_unprotect(&raw)?)?)
}
