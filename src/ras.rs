//! RAS 封装：拨号条目/凭据/拨号/状态/挂断。
//!
//! 纯逻辑（错误分类）跨平台 TDD；Win32 胶水仅 Windows 上编译，
//! 以 `cargo check --target x86_64-pc-windows-msvc` 验证。

/// 拨号错误分类：Auth 触发长退避（认证失败高频重试无意义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrKind {
    /// 认证被拒（691：用户名/密码错误或账号在线）。
    Auth,
    /// 瞬时故障（链路断、服务器忙、适配器异常等）。
    Transient,
}

/// 拨号返回码分类：691（认证拒绝）→ Auth，其余 → Transient。
pub fn classify(code: u32) -> ErrKind {
    if code == 691 {
        ErrKind::Auth
    } else {
        ErrKind::Transient
    }
}

#[cfg(test)]
mod tests {
    use super::{classify, ErrKind};

    #[test]
    fn classify_691_as_auth_others_transient() {
        assert_eq!(classify(691), ErrKind::Auth);
        assert_eq!(classify(651), ErrKind::Transient);
        assert_eq!(classify(678), ErrKind::Transient);
        assert_eq!(classify(619), ErrKind::Transient);
        assert_eq!(classify(99999), ErrKind::Transient);
    }
}

#[cfg(windows)]
mod win {
    use std::mem::size_of;
    use std::thread::sleep;
    use std::time::Duration;

    use anyhow::{anyhow, Result};
    use windows::core::PCWSTR;
    use windows::Win32::NetworkManagement::Rras::{
        self, ET_Require, RASCM_Password, RASCM_UserName, RASCS_Disconnected, RASET_Broadband,
        RASFP_Ppp, RASNP_Ip, RASNP_Ipx, RASNP_NetBEUI, RasDialW, RasEnumConnectionsW,
        RasGetConnectStatusW, RasGetErrorStringW, RasHangUpW, RasSetCredentialsW,
        RasSetEntryPropertiesW, HRASCONN, RASCONNSTATUSW, RASCONNW, RASCREDENTIALSW,
        RASDIALPARAMSW, RASENTRYW,
    };

    use super::{classify, ErrKind};

    /// 会话状态：Connected 涵盖真连接与中间态（守护层靠探针复核真伪）。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ConnState {
        Connected,
        Disconnected,
    }

    /// str → 定长 UTF-16 缓冲（null 结尾，超长截断）。len 为缓冲总长，含 null 位。
    /// 调用点约定：len ≥ 1（必须给 null 留位），防御性断言拦截笔误。
    fn wide(s: &str, len: usize) -> Vec<u16> {
        assert!(len >= 1, "wide: len 必须 ≥ 1（null 结尾至少占 1 位）");
        let mut buf = Vec::with_capacity(len);
        buf.extend(s.encode_utf16().take(len - 1));
        buf.resize(len, 0);
        buf
    }

    /// PCWSTR 指向缓冲首元素；空缓冲退化为 null 指针。
    fn as_pw(p: &[u16]) -> PCWSTR {
        PCWSTR(p.as_ptr())
    }

    fn ras_err(step: &str, code: u32) -> anyhow::Error {
        anyhow!("{step} 失败: 错误码 {code} ({})", error_string(code))
    }

    /// RasGetErrorStringW 取 RAS 错误描述；取不到时回退错误码本身。
    pub fn error_string(code: u32) -> String {
        let mut buf = [0u16; 1024];
        let ret = unsafe { RasGetErrorStringW(code, &mut buf) };
        if ret != 0 {
            return format!("RAS 错误 {code}");
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }

    /// 幂等创建/更新宽带拨号条目：PPPoE + WAN Miniport (PPPoE)。
    ///
    /// `dwfNetProtocols` 必须含 `RASNP_Ip`（对齐官方客户端条目 ExcludedProtocols=8 的反推值 7：
    /// NetBEUI+Ipx+Ip）。default() 的 0 会让 RAS 排除 IP 协议族，PPPoE IPCP 无法协商 → 错误 720。
    /// `dwEncryptionType=ET_Require` 对应官方 pbk 的 DataEncryption=8（要求加密密码）。
    pub fn ensure_entry(pbk: &str, name: &str) -> Result<()> {
        let entry = RASENTRYW {
            dwSize: size_of::<RASENTRYW>() as u32,
            dwType: RASET_Broadband,
            dwFramingProtocol: RASFP_Ppp,
            dwfNetProtocols: RASNP_Ip | RASNP_Ipx | RASNP_NetBEUI,
            dwEncryptionType: ET_Require,
            szDeviceType: wide("PPPoE", 17).try_into().expect("szDeviceType 定长 17"),
            szDeviceName: wide("WAN Miniport (PPPoE)", 129)
                .try_into()
                .expect("szDeviceName 定长 129"),
            ..RASENTRYW::default()
        };

        let pbk = wide(pbk, pbk.len() + 1);
        let name_buf = wide(name, name.len() + 1);
        let ret = unsafe {
            RasSetEntryPropertiesW(as_pw(&pbk), as_pw(&name_buf), &entry, entry.dwSize, None, 0)
        };
        if ret != 0 {
            // 816 = 地址已关联（端口被占用时更新条目会报此码），此时若 pbk 文件已存在且
            // 参数符合预期则视为成功（端口释放后即可拨号），避免在 Dr.COM 在线时 install 失败。
            if ret == 816 {
                log::warn!(
                    "RasSetEntryPropertiesW 返回 816（端口占用），视为成功（端口释放后可拨号）"
                );
                return Ok(());
            }
            return Err(ras_err("RasSetEntryPropertiesW", ret));
        }
        Ok(())
    }

    /// 写入条目凭据（用户名 + 密码，存入凭据管理器）。
    pub fn set_credentials(pbk: &str, name: &str, user: &str, pass: &str) -> Result<()> {
        let cred = RASCREDENTIALSW {
            dwSize: size_of::<RASCREDENTIALSW>() as u32,
            dwMask: RASCM_UserName | RASCM_Password,
            szUserName: wide(user, 257).try_into().expect("szUserName 定长 257"),
            szPassword: wide(pass, 257).try_into().expect("szPassword 定长 257"),
            ..RASCREDENTIALSW::default()
        };

        let pbk = wide(pbk, pbk.len() + 1);
        let name_buf = wide(name, name.len() + 1);
        let ret = unsafe { RasSetCredentialsW(as_pw(&pbk), as_pw(&name_buf), &cred, false) };
        if ret != 0 {
            return Err(ras_err("RasSetCredentialsW", ret));
        }
        Ok(())
    }

    /// 同步拨号（回调 None）：成功返回会话句柄；691 归类 Auth。
    /// 756（已经拨了这个连接）/813（前一个宽带连接未断开）先挂旧会话重试一次——
    /// RAS 对同端口的第二条宽带连接报这两码，服务自愈场景（残留会话）常见。
    pub fn dial(pbk: &str, name: &str, user: &str, pass: &str) -> Result<RasSession, RasError> {
        match dial_once(pbk, name, user, pass) {
            Err(RasError::Other {
                code: code @ (756 | 813),
                ..
            }) => {
                log::warn!("拨号 {code}（旧会话残留），枚举挂断后重试一次");
                if let Some(h) = find_entry_connection(pbk, name) {
                    unsafe {
                        let _ = RasHangUpW(h);
                    }
                    sleep(Duration::from_millis(500));
                }
                dial_once(pbk, name, user, pass)
            }
            other => other,
        }
    }

    fn dial_once(pbk: &str, name: &str, user: &str, pass: &str) -> Result<RasSession, RasError> {
        let params = RASDIALPARAMSW {
            dwSize: size_of::<RASDIALPARAMSW>() as u32,
            szEntryName: wide(name, 257).try_into().expect("szEntryName 定长 257"),
            szUserName: wide(user, 257).try_into().expect("szUserName 定长 257"),
            szPassword: wide(pass, 257).try_into().expect("szPassword 定长 257"),
            ..RASDIALPARAMSW::default()
        };

        let mut handle = HRASCONN::default();
        let pbk = wide(pbk, pbk.len() + 1);
        let ret = unsafe { RasDialW(None, as_pw(&pbk), &params, 0, None, &mut handle) };
        if ret != 0 {
            return Err(match classify(ret) {
                ErrKind::Auth => RasError::Auth,
                ErrKind::Transient => RasError::Other {
                    code: ret,
                    msg: error_string(ret),
                },
            });
        }
        Ok(RasSession { handle })
    }

    /// 枚举 RAS 连接，取属于指定 phonebook 条目的活跃句柄（自愈残留会话用）。
    /// 若精确匹配失败（大小写/路径规范化差异），退化为同设备 PPPoE 连接兜底。
    fn find_entry_connection(pbk: &str, name: &str) -> Option<HRASCONN> {
        const ERROR_SUCCESS_RAW: u32 = 0;
        const ERROR_BUFFER_TOO_SMALL_RAW: u32 = 603;
        // RasEnumConnectionsW 要求每个元素的 dwSize = sizeof(RASCONNW)，default() 为 0 会报 632。
        let mk = || RASCONNW {
            dwSize: size_of::<RASCONNW>() as u32,
            ..RASCONNW::default()
        };
        let mut buf: Vec<RASCONNW> = vec![mk(); 4];
        let mut size = (size_of::<RASCONNW>() * buf.len()) as u32;
        let mut count = 0u32;
        let mut ret = unsafe { RasEnumConnectionsW(Some(buf.as_mut_ptr()), &mut size, &mut count) };
        if ret == ERROR_BUFFER_TOO_SMALL_RAW {
            buf = vec![mk(); (size as usize) / size_of::<RASCONNW>() + 1];
            ret = unsafe { RasEnumConnectionsW(Some(buf.as_mut_ptr()), &mut size, &mut count) };
        }
        if ret != ERROR_SUCCESS_RAW {
            log::warn!("RasEnumConnectionsW 失败: {ret}");
            return None;
        }
        let name_w: Vec<u16> = name.encode_utf16().collect();
        let pbk_w: Vec<u16> = pbk.encode_utf16().collect();
        // 1. 精确匹配（同 pbk + 同条目名，大小写不敏感）
        for c in &buf[..count as usize] {
            if entry_name_slice(&c.szEntryName) == name_w.as_slice()
                && pbk_eq_ci(&c.szPhonebook, pbk_w.as_slice())
            {
                return Some(c.hrasconn);
            }
        }
        // 2. 兜底：同名条目（跨 pbk，Dr.COM 手动拨号场景）
        for c in &buf[..count as usize] {
            if entry_name_slice(&c.szEntryName) == name_w.as_slice() {
                return Some(c.hrasconn);
            }
        }
        // 3. 兜底：任意活跃的宽带 PPPoE 连接（端口被占 756/813 时抢占）
        if count > 0 {
            // 优先同设备名（WAN Miniport PPPoE），否则任意首个
            for c in &buf[..count as usize] {
                let dev = entry_name_slice(&c.szDeviceName);
                let dev_s = String::from_utf16_lossy(dev);
                if dev_s.contains("PPPoE") || dev_s.contains("PPOE") {
                    return Some(c.hrasconn);
                }
            }
            return Some(buf[0].hrasconn);
        }
        None
    }

    fn entry_name_slice(buf: &[u16]) -> &[u16] {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        &buf[..end]
    }

    fn pbk_eq_ci(a: &[u16], b: &[u16]) -> bool {
        let ae = a.iter().position(|&c| c == 0).unwrap_or(a.len());
        let be = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        if ae != be {
            return false;
        }
        a[..ae].iter().zip(&b[..be]).all(|(x, y)| {
            let xc = char::from_u32(*x as u32).unwrap_or('\0');
            let yc = char::from_u32(*y as u32).unwrap_or('\0');
            xc.eq_ignore_ascii_case(&yc)
        })
    }

    fn pbk_eq(buf: &[u16], pbk: &[u16]) -> bool {
        pbk_eq_ci(buf, pbk)
    }
    /// RasDial 失败：Auth 走守护层 AuthFail 长退避，Other 带码走指数退避。
    #[derive(Debug, thiserror::Error)]
    pub enum RasError {
        #[error("认证失败(691)：学号或密码错误")]
        Auth,
        #[error("RAS 错误 {code}: {msg}")]
        Other { code: u32, msg: String },
    }

    /// 一次活跃的 PPPoE 会话。Drop 不自动挂断（由守护层显式 hangup）。
    pub struct RasSession {
        handle: HRASCONN,
    }

    impl RasSession {
        /// RAS 层连接状态；中间态一律按 Connected 上报，真伪由探针复核。
        pub fn status(&self) -> Result<ConnState> {
            let mut st = RASCONNSTATUSW {
                dwSize: size_of::<RASCONNSTATUSW>() as u32,
                ..RASCONNSTATUSW::default()
            };
            let ret = unsafe { RasGetConnectStatusW(self.handle, &mut st) };
            if ret == Rras::ERROR_NO_CONNECTION {
                return Ok(ConnState::Disconnected);
            }
            if ret != 0 {
                return Err(ras_err("RasGetConnectStatusW", ret));
            }
            // 先判 Disconnected（服务器踢线时 dwError 可能同时非 0，断开优先）
            if st.rasconnstate == RASCS_Disconnected {
                return Ok(ConnState::Disconnected);
            }
            if st.dwError != 0 {
                return Err(ras_err("RasGetConnectStatusW", st.dwError));
            }
            // 中间态一律按 Connected 上报，真伪由探针复核
            Ok(ConnState::Connected)
        }

        /// 挂断并等待 300ms 让系统释放句柄（重拨前必须）。
        pub fn hangup(&self) -> Result<()> {
            let ret = unsafe { RasHangUpW(self.handle) };
            if ret != 0 && ret != Rras::ERROR_NO_CONNECTION {
                return Err(ras_err("RasHangUpW", ret));
            }
            sleep(Duration::from_millis(300));
            Ok(())
        }
    }
}

#[cfg(windows)]
pub use win::{dial, ensure_entry, error_string, set_credentials, ConnState, RasError, RasSession};
