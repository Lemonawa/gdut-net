//! DPAPI 密码保护：`GDUT1:<entropy_hex>:<base64>` blob 与 Windows DPAPI 胶水。
//!
//! 纯逻辑（blob 编解码）跨平台 TDD；DPAPI/注册表仅 Windows 上编译，
//! 以 `cargo check --target x86_64-pc-windows-msvc` 验证。

const BLOB_PREFIX: &str = "GDUT1:";
#[cfg(windows)]
const ENTROPY_LEN: usize = 32;
#[cfg(windows)]
const REG_SUBKEY: &str = r"SOFTWARE\gdut-net";
#[cfg(windows)]
const REG_VALUE: &str = "entropy";

/// 编码 blob：`GDUT1:<entropy_hex>:<base64(protected)>`。
pub fn wrap_blob(entropy_hex: &str, protected: &[u8]) -> String {
    use base64::Engine as _;
    format!(
        "{BLOB_PREFIX}{entropy_hex}:{}",
        base64::engine::general_purpose::STANDARD.encode(protected)
    )
}

/// 解码 blob：前缀 `GDUT1:`，entropy 十六进制（偶数位），密文 base64；任一环节非法返回 None。
pub fn unwrap_blob(s: &str) -> Option<(String, Vec<u8>)> {
    use base64::Engine as _;
    let rest = s.strip_prefix(BLOB_PREFIX)?;
    let (entropy_hex, protected_b64) = rest.split_once(':')?;
    if entropy_hex.is_empty()
        || entropy_hex.len() % 2 != 0
        || !entropy_hex.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    let protected = base64::engine::general_purpose::STANDARD
        .decode(protected_b64)
        .ok()?;
    if protected.is_empty() {
        return None;
    }
    Some((entropy_hex.to_ascii_lowercase(), protected))
}

#[cfg(windows)]
mod win {
    use anyhow::{anyhow, Context, Result};
    use rand::RngExt as _;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{
        LocalFree, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, HLOCAL, WIN32_ERROR,
    };
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_LOCAL_MACHINE, CRYPT_INTEGER_BLOB,
    };
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegGetValueW, RegSetValueExW, HKEY,
        HKEY_LOCAL_MACHINE, KEY_WRITE, REG_BINARY, REG_OPTION_NON_VOLATILE, REG_VALUE_TYPE,
        RRF_RT_REG_BINARY,
    };

    use super::{unwrap_blob, wrap_blob, ENTROPY_LEN, REG_SUBKEY, REG_VALUE};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// PW 指针借用缓冲； SAFETY 注释点约束调用方：指针仅在缓冲存活期间使用。
    fn pw(buf: &[u16]) -> PCWSTR {
        PCWSTR(buf.as_ptr())
    }

    fn win32_err(step: &str, code: WIN32_ERROR) -> anyhow::Error {
        anyhow!("{step} 失败: 错误码 {}", code.0)
    }

    /// 读取 HKLM\SOFTWARE\gdut-net 下 entropy 值；不存在/损坏则生成 32B 随机并写入。
    pub fn ensure_entropy() -> Result<Vec<u8>> {
        let subkey = wide(REG_SUBKEY);
        let value = wide(REG_VALUE);

        let mut buf = [0u8; ENTROPY_LEN];
        let mut size = buf.len() as u32;
        let mut vtype = REG_VALUE_TYPE::default();
        let ret = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                pw(&subkey),
                pw(&value),
                RRF_RT_REG_BINARY,
                Some(&mut vtype),
                Some(buf.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if ret == ERROR_SUCCESS && size as usize == ENTROPY_LEN {
            return Ok(buf.to_vec());
        }

        let mut entropy = [0u8; ENTROPY_LEN];
        rand::rng().fill(&mut entropy);

        let mut hkey = HKEY::default();
        let ret = unsafe {
            RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                pw(&subkey),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut hkey,
                None,
            )
        };
        if ret != ERROR_SUCCESS {
            return Err(win32_err("RegCreateKeyExW", ret));
        }
        let ret = unsafe { RegSetValueExW(hkey, pw(&value), None, REG_BINARY, Some(&entropy)) };
        let closed = unsafe { RegCloseKey(hkey) };
        if ret != ERROR_SUCCESS {
            return Err(win32_err("RegSetValueExW", ret));
        }
        if closed != ERROR_SUCCESS {
            return Err(win32_err("RegCloseKey", closed));
        }
        Ok(entropy.to_vec())
    }

    /// 删除 HKLM\SOFTWARE\gdut-net 整棵键（uninstall 用）；键不存在视为已删除。
    pub fn delete_entropy() -> Result<()> {
        let ret = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, pw(&wide(REG_SUBKEY))) };
        if ret != ERROR_SUCCESS && ret != ERROR_FILE_NOT_FOUND {
            return Err(win32_err("RegDeleteTreeW", ret));
        }
        Ok(())
    }

    /// DPAPI 机器级加密：随机 32B entropy（注册表存 REG_BINARY）+ CryptProtectData，产 blob。
    pub fn protect(plain: &str) -> Result<String> {
        let entropy = ensure_entropy()?;

        let mut out = CRYPT_INTEGER_BLOB::default();
        let ret = unsafe {
            CryptProtectData(
                &CRYPT_INTEGER_BLOB {
                    cbData: plain.len() as u32,
                    pbData: plain.as_ptr().cast_mut(),
                },
                PCWSTR::null(),
                Some(&CRYPT_INTEGER_BLOB {
                    cbData: entropy.len() as u32,
                    pbData: entropy.as_ptr().cast_mut(),
                }),
                None,
                None,
                CRYPTPROTECT_LOCAL_MACHINE,
                &mut out,
            )
        };
        if let Err(e) = ret {
            return Err(anyhow!("CryptProtectData 失败: {e}"));
        }
        // SAFETY：cbData/pbData 由 DPAPI 填充；拷贝必须在 LocalFree 之前完成。
        let protected = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) };
        let blob = wrap_blob(&hex_encode(&entropy), protected);
        unsafe { LocalFree(Some(HLOCAL(out.pbData.cast()))) };
        Ok(blob)
    }

    /// 解 blob 并 DPAPI 解密；entropy 不匹配或密文损坏均报错。
    pub fn unprotect(blob: &str) -> Result<String> {
        let (entropy_hex, protected) =
            unwrap_blob(blob).context("blob 格式非法（应为 GDUT1:<hex>:<base64>）")?;
        let entropy = hex_decode(&entropy_hex)?;

        let mut out = CRYPT_INTEGER_BLOB::default();
        let ret = unsafe {
            CryptUnprotectData(
                &CRYPT_INTEGER_BLOB {
                    cbData: protected.len() as u32,
                    pbData: protected.as_ptr().cast_mut(),
                },
                None,
                Some(&CRYPT_INTEGER_BLOB {
                    cbData: entropy.len() as u32,
                    pbData: entropy.as_ptr().cast_mut(),
                }),
                None,
                None,
                0,
                &mut out,
            )
        };
        if let Err(e) = ret {
            return Err(anyhow!("CryptUnprotectData 失败: {e}"));
        }
        // SAFETY：同 protect；拷贝必须在 LocalFree 之前完成。
        let plain_bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) };
        let plain = String::from_utf8(plain_bytes.to_vec())
            .context("解密结果不是合法 UTF-8（blob 损坏或 entropy 不匹配）");
        unsafe { LocalFree(Some(HLOCAL(out.pbData.cast()))) };
        plain
    }

    fn hex_encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn hex_decode(s: &str) -> Result<Vec<u8>> {
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(Into::into))
            .collect()
    }
}

#[cfg(windows)]
pub use win::{delete_entropy, ensure_entropy, protect, unprotect};

#[cfg(test)]
mod tests {
    use super::{unwrap_blob, wrap_blob};

    #[test]
    fn blob_roundtrip_preserves_hex_normalization() {
        let b = wrap_blob("AaBb01Ff", &[0xde, 0xad]);
        let (e, p) = unwrap_blob(&b).unwrap();
        assert_eq!(e, "aabb01ff");
        assert_eq!(p, vec![0xde, 0xad]);
    }

    #[test]
    fn blob_rejects_bad_hex_and_base64() {
        assert!(unwrap_blob("GDUT1:zz:x").is_none());
        assert!(unwrap_blob("GDUT1:abc").is_none());
        assert!(unwrap_blob("GDUT1:aa:!!!").is_none());
        assert!(unwrap_blob("GDUT1:aa:").is_none());
    }
}
