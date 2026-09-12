//! Win32 原语：宽字符串与注册表写入的唯一实现。
//!
//! `wide` 是纯函数（Linux 可测）；注册表部分仅 Windows，错误信息带值名与
//! 返回码，便于真机排障。

/// str → UTF-16 + NUL（Win32 宽字符参数）。
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
pub mod reg {
    use anyhow::{bail, Result};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, KEY_WRITE, REG_DWORD,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    use super::wide;

    fn create(root: HKEY, subkey: &str) -> Result<HKEY> {
        let subkey_w = wide(subkey);
        let mut hkey = HKEY::default();
        let ret = unsafe {
            RegCreateKeyExW(
                root,
                PCWSTR(subkey_w.as_ptr()),
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
            bail!("RegCreateKeyExW({subkey}) failed: {}", ret.0);
        }
        Ok(hkey)
    }

    fn set_value(
        root: HKEY,
        subkey: &str,
        name: Option<&str>,
        value_type: windows::Win32::System::Registry::REG_VALUE_TYPE,
        bytes: &[u8],
    ) -> Result<()> {
        let hkey = create(root, subkey)?;
        let name_w = name.map(wide);
        let ret = unsafe {
            RegSetValueExW(
                hkey,
                name_w
                    .as_ref()
                    .map_or(PCWSTR::null(), |w| PCWSTR(w.as_ptr())),
                None,
                value_type,
                Some(bytes),
            )
        };
        let _ = unsafe { RegCloseKey(hkey) };
        if ret != ERROR_SUCCESS {
            bail!(
                "RegSetValueExW({}\\{}) failed: {}",
                subkey,
                name.unwrap_or("(default)"),
                ret.0
            );
        }
        Ok(())
    }

    /// 建/开键后写一个 REG_SZ；`name=None` 写默认值。
    pub fn set_string(root: HKEY, subkey: &str, name: Option<&str>, value: &str) -> Result<()> {
        let bytes: Vec<u8> = wide(value).iter().flat_map(|c| c.to_le_bytes()).collect();
        set_value(root, subkey, name, REG_SZ, &bytes)
    }

    /// 写一个 REG_DWORD。
    pub fn set_dword(root: HKEY, subkey: &str, name: &str, value: u32) -> Result<()> {
        set_value(root, subkey, Some(name), REG_DWORD, &value.to_le_bytes())
    }
}
