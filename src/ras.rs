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
        self, RASCM_Password, RASCM_UserName, RASCS_Disconnected, RASET_Broadband, RASFP_Ppp,
        RasDialW, RasGetConnectStatusW, RasGetErrorStringW, RasHangUpW, RasSetCredentialsW,
        RasSetEntryPropertiesW, HRASCONN, RASCONNSTATUSW, RASCREDENTIALSW, RASDIALPARAMSW,
        RASENTRYW,
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
    pub fn ensure_entry(pbk: &str, name: &str) -> Result<()> {
        let entry = RASENTRYW {
            dwSize: size_of::<RASENTRYW>() as u32,
            dwType: RASET_Broadband,
            dwFramingProtocol: RASFP_Ppp,
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
    pub fn dial(pbk: &str, name: &str, user: &str, pass: &str) -> Result<RasSession, RasError> {
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
