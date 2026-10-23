//! Snapshot only values owned by an install step, preserving unrelated values.

use anyhow::{Context, Result};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::*;

use crate::win32::wide;

type SavedValue = Option<(REG_VALUE_TYPE, Vec<u8>)>;

pub(crate) struct RegistryBackup {
    root: HKEY,
    key: String,
    existed: bool,
    values: Vec<(String, SavedValue)>,
}

impl RegistryBackup {
    pub(crate) fn capture(root: HKEY, key: &str, names: &[&str]) -> Result<Self> {
        let key_w = wide(key);
        let mut handle = HKEY::default();
        let status =
            unsafe { RegOpenKeyExW(root, PCWSTR(key_w.as_ptr()), None, KEY_READ, &mut handle) };
        let existed = status == ERROR_SUCCESS;
        anyhow::ensure!(
            existed || status == ERROR_FILE_NOT_FOUND,
            "Cannot inspect registry key {key}: {}",
            status.0
        );
        if existed {
            let _ = unsafe { RegCloseKey(handle) };
        }
        let values = names
            .iter()
            .map(|name| Ok((name.to_string(), read_value(root, key, name)?)))
            .collect::<Result<_>>()?;
        Ok(Self {
            root,
            key: key.into(),
            existed,
            values,
        })
    }

    pub(crate) fn restore(&self) -> Result<()> {
        let key = wide(&self.key);
        let mut handle = HKEY::default();
        let status = unsafe {
            RegCreateKeyExW(
                self.root,
                PCWSTR(key.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE | KEY_QUERY_VALUE,
                None,
                &mut handle,
                None,
            )
        };
        anyhow::ensure!(
            status == ERROR_SUCCESS,
            "Cannot open registry key for rollback: {}",
            status.0
        );
        let result = (|| -> Result<()> {
            let mut failures = Vec::new();
            for (name, before) in &self.values {
                let name_w = wide(name);
                let status = unsafe {
                    match before {
                        Some((kind, bytes)) => RegSetValueExW(
                            handle,
                            PCWSTR(name_w.as_ptr()),
                            None,
                            *kind,
                            Some(bytes),
                        ),
                        None => RegDeleteValueW(handle, PCWSTR(name_w.as_ptr())),
                    }
                };
                if status != ERROR_SUCCESS && !(before.is_none() && status == ERROR_FILE_NOT_FOUND)
                {
                    failures.push(format!("{name}: {}", status.0));
                }
            }
            anyhow::ensure!(
                failures.is_empty(),
                "Registry rollback failed: {}",
                failures.join("; ")
            );
            Ok(())
        })();
        let mut subkeys = 0;
        let mut values = 0;
        let query = unsafe {
            RegQueryInfoKeyW(
                handle,
                None,
                None,
                None,
                Some(&mut subkeys),
                None,
                None,
                Some(&mut values),
                None,
                None,
                None,
                None,
            )
        };
        let _ = unsafe { RegCloseKey(handle) };
        result?;
        if !self.existed {
            anyhow::ensure!(
                query == ERROR_SUCCESS,
                "Cannot inspect restored registry key: {}",
                query.0
            );
            // Do not delete keys containing unrelated user values or subkeys.
            if subkeys == 0 && values == 0 {
                let status = unsafe { RegDeleteKeyW(self.root, PCWSTR(key.as_ptr())) };
                anyhow::ensure!(
                    status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND,
                    "Cannot remove newly created registry key: {}",
                    status.0
                );
            }
        }
        Ok(())
    }
}

fn read_value(root: HKEY, key: &str, name: &str) -> Result<SavedValue> {
    let key = wide(key);
    let name = wide(name);
    let mut kind = REG_VALUE_TYPE::default();
    let mut len = 0;
    let status = unsafe {
        RegGetValueW(
            root,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_ANY | RRF_NOEXPAND,
            Some(&mut kind),
            None,
            Some(&mut len),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    anyhow::ensure!(
        status == ERROR_SUCCESS,
        "Cannot size registry backup: {}",
        status.0
    );
    let mut bytes = vec![0; len as usize];
    let status = unsafe {
        RegGetValueW(
            root,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_ANY | RRF_NOEXPAND,
            Some(&mut kind),
            Some(bytes.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    anyhow::ensure!(
        status == ERROR_SUCCESS,
        "Cannot read registry backup: {}",
        status.0
    );
    bytes.truncate(usize::try_from(len).context("Registry value size exceeds platform range")?);
    Ok(Some((kind, bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestKey(String);
    impl TestKey {
        fn new() -> Self {
            Self(format!(
                r"Software\gdut-net-tests\backup-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ))
        }
        fn set(&self, name: &str, kind: REG_VALUE_TYPE, bytes: &[u8]) {
            let key = wide(&self.0);
            let name = wide(name);
            let mut handle = HKEY::default();
            assert_eq!(
                unsafe {
                    RegCreateKeyExW(
                        HKEY_CURRENT_USER,
                        PCWSTR(key.as_ptr()),
                        None,
                        PCWSTR::null(),
                        REG_OPTION_NON_VOLATILE,
                        KEY_SET_VALUE,
                        None,
                        &mut handle,
                        None,
                    )
                },
                ERROR_SUCCESS
            );
            assert_eq!(
                unsafe { RegSetValueExW(handle, PCWSTR(name.as_ptr()), None, kind, Some(bytes)) },
                ERROR_SUCCESS
            );
            let _ = unsafe { RegCloseKey(handle) };
        }
        fn get(&self, name: &str) -> Option<(REG_VALUE_TYPE, Vec<u8>)> {
            read_value(HKEY_CURRENT_USER, &self.0, name).unwrap()
        }
    }
    impl Drop for TestKey {
        fn drop(&mut self) {
            let key = wide(&self.0);
            let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr())) };
        }
    }

    #[test]
    fn restores_raw_expand_string_and_preserves_unrelated_values() {
        let key = TestKey::new();
        let original: Vec<u8> = wide("%TEMP%\\old.exe")
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        key.set("owned", REG_EXPAND_SZ, &original);
        key.set("unrelated", REG_BINARY, b"keep");
        let backup =
            RegistryBackup::capture(HKEY_CURRENT_USER, &key.0, &["owned", "added"]).unwrap();
        key.set("owned", REG_BINARY, b"changed");
        key.set("added", REG_DWORD, &1u32.to_le_bytes());
        backup.restore().unwrap();
        assert_eq!(key.get("owned"), Some((REG_EXPAND_SZ, original)));
        assert!(key.get("added").is_none());
        assert_eq!(key.get("unrelated"), Some((REG_BINARY, b"keep".to_vec())));
    }

    #[test]
    fn newly_created_key_is_removed_after_owned_values_are_removed() {
        let key = TestKey::new();
        let backup = RegistryBackup::capture(HKEY_CURRENT_USER, &key.0, &["owned"]).unwrap();
        key.set("owned", REG_BINARY, b"new");
        backup.restore().unwrap();
        let key_w = wide(&key.0);
        let mut handle = HKEY::default();
        assert_eq!(
            unsafe {
                RegOpenKeyExW(
                    HKEY_CURRENT_USER,
                    PCWSTR(key_w.as_ptr()),
                    None,
                    KEY_READ,
                    &mut handle,
                )
            },
            ERROR_FILE_NOT_FOUND
        );
    }

    #[test]
    fn newly_created_key_survives_when_someone_added_an_unrelated_value() {
        let key = TestKey::new();
        let backup = RegistryBackup::capture(HKEY_CURRENT_USER, &key.0, &["owned"]).unwrap();
        key.set("owned", REG_BINARY, b"new");
        key.set("unrelated", REG_BINARY, b"keep");
        backup.restore().unwrap();
        assert!(key.get("owned").is_none());
        assert_eq!(key.get("unrelated"), Some((REG_BINARY, b"keep".to_vec())));
    }
}
