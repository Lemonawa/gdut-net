//! 无线接管（ADR-0005）：纯决策核心 + Win32 胶水分层（同 probe.rs 先例）。

pub mod portal;
#[cfg(windows)]
pub mod routes;
#[cfg(windows)]
pub mod test;
#[cfg(windows)]
pub mod wlan;

use crate::ipc::protocol::{NetMode, WPhase, WirelessSnapshot};
use crate::probe::ProbeVerdict;

/// portal 认证失败重试间隔（秒）：5s → 15s → 30s 封顶。
pub const AUTH_RETRY_DELAYS: [u64; 3] = [5, 15, 30];
/// Joining 相位等 IP/关联超时（manager 判定后调 restart）。
pub const JOIN_TIMEOUT_SECS: u64 = 60;

/// IPv4 → Windows `SOCKADDR_IN.sin_addr.S_addr` 期望的数值表示。
///
/// Win32 要求 S_addr 的内存字节为网络序（点分顺序）。`u32::from(ip)` 是
/// 大端数值（10.0.3.2 → 0x0A000302），直接存入 LE 内存会变成字节
/// [02,03,00,0A] → 路由表里出现 2.3.0.10（2026-09-10 真机实测翻车）。
/// `to_be()` 在 LE 上交换字节、在 BE 上原样，两端都得到正确的内存布局。
pub fn ipv4_to_s_addr(ip: std::net::Ipv4Addr) -> u32 {
    u32::from(ip).to_be()
}

/// [`ipv4_to_s_addr`] 的逆：S_addr 数值 → IPv4。
pub fn s_addr_to_ipv4(v: u32) -> std::net::Ipv4Addr {
    std::net::Ipv4Addr::from(v.to_be())
}

/// manager 每拍采集的世界状态（纯数据）。
#[derive(Debug, Clone, Default)]
pub struct World {
    pub now: u64,
    pub eth_link_up: bool,
    pub wired_connected: bool,
    pub wlan_associated: bool,
    pub wlan_ip: bool,
    pub probe: Option<ProbeVerdict>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    /// WlanConnect(profile)。
    Associate,
    /// WlanDisconnect + 路由/metric 回滚（manager 统一执行）。
    Disassociate,
    /// 发一次 eportal 登录，结果经 [`Brain::on_auth`] 回报。
    PortalAuth,
    /// 到探测周期，manager 执行 probe_once 并缓存进下一拍 World.probe。
    ProbeNow,
}

/// 纯决策核心（ADR-0005 状态机）。不碰系统，只出 Action。
pub struct Brain {
    mode: NetMode,
    phase: WPhase,
    unhealthy_since: Option<u64>,
    healthy_since: Option<u64>,
    last_probe_at: Option<u64>,
    auth_busy: bool,
    auth_fail_count: u32,
    next_auth_at: Option<u64>,
    last_error: Option<String>,
    takeover_after: u64,
    release_after: u64,
    probe_interval: u64,
}

impl Brain {
    pub fn new(
        mode: NetMode,
        takeover_after: u64,
        release_after: u64,
        probe_interval: u64,
    ) -> Self {
        Self {
            mode,
            phase: WPhase::Off,
            unhealthy_since: None,
            healthy_since: None,
            last_probe_at: None,
            auth_busy: false,
            auth_fail_count: 0,
            next_auth_at: None,
            last_error: None,
            takeover_after,
            release_after,
            probe_interval,
        }
    }

    pub fn phase(&self) -> WPhase {
        self.phase
    }

    pub fn set_mode(&mut self, m: NetMode) {
        self.mode = m;
    }

    /// Joining 超时/外部复位：回 Off（带外计数清零）。
    pub fn restart(&mut self) {
        self.reset_off();
    }

    pub fn on_auth(&mut self, ok: bool, msg: &str, now: u64) {
        self.auth_busy = false;
        if ok {
            self.phase = WPhase::Online;
            self.auth_fail_count = 0;
            self.next_auth_at = None;
            self.last_error = None;
            self.last_probe_at = None;
        } else {
            self.auth_fail_count = self.auth_fail_count.saturating_add(1);
            let idx = (self.auth_fail_count as usize - 1).min(AUTH_RETRY_DELAYS.len() - 1);
            self.next_auth_at = Some(now + AUTH_RETRY_DELAYS[idx]);
            self.phase = WPhase::Error;
            self.last_error = Some(msg.to_string());
        }
    }

    pub fn snapshot(&self) -> WirelessSnapshot {
        WirelessSnapshot {
            phase: self.phase,
            ip: None,
            last_error: self.last_error.clone(),
        }
    }

    pub fn decide(&mut self, w: &World) -> Action {
        match self.mode {
            NetMode::WiredPlusStandby => self.keep_wireless(w),
            NetMode::WiredExclusive => {
                // spec §5：链路态是秒级快信号，watchdog 是兜底（会话死但
                // 链路活）。两者任一缺失即不健康；同时就绪才算健康。
                if w.eth_link_up && w.wired_connected {
                    self.unhealthy_since = None;
                    let since = *self.healthy_since.get_or_insert(w.now);
                    if w.now - since >= self.release_after && self.phase != WPhase::Off {
                        self.reset_off();
                        return Action::Disassociate;
                    }
                    Action::None
                } else {
                    self.healthy_since = None;
                    let since = *self.unhealthy_since.get_or_insert(w.now);
                    let debounced = w.now - since >= self.takeover_after;
                    match self.phase {
                        WPhase::Off if debounced => self.start_join(),
                        WPhase::Off => Action::None,
                        WPhase::Error if self.next_auth_at.is_none_or(|t| w.now >= t) => {
                            self.start_join()
                        }
                        WPhase::Error => Action::None,
                        _ => self.keep_wireless(w),
                    }
                }
            }
        }
    }

    /// 无线侧维持逻辑（standby 全量 / exclusive 失联期）。
    fn keep_wireless(&mut self, w: &World) -> Action {
        match self.phase {
            WPhase::Off => self.start_join(),
            WPhase::Error if self.next_auth_at.is_none_or(|t| w.now >= t) => self.start_join(),
            WPhase::Error => Action::None,
            WPhase::Joining => {
                if w.wlan_associated && w.wlan_ip {
                    self.phase = WPhase::Authing;
                    self.auth_busy = true;
                    self.next_auth_at = None;
                    Action::PortalAuth
                } else {
                    Action::None
                }
            }
            WPhase::Authing => {
                if !self.auth_busy && self.next_auth_at.is_none_or(|t| w.now >= t) {
                    self.auth_busy = true;
                    Action::PortalAuth
                } else {
                    Action::None
                }
            }
            WPhase::Online => {
                if w.probe == Some(ProbeVerdict::Kicked) {
                    self.phase = WPhase::Authing;
                    self.auth_busy = true;
                    self.next_auth_at = None;
                    return Action::PortalAuth;
                }
                // 关联在但适配器层面无 IP（探测结论之外的自愈路径：探测
                // verdict 可能抖动，IP 缺失是确定性事实）→ 重走 Joining，
                // 由 60s join 超时兜底。
                if !w.wlan_associated || !w.wlan_ip {
                    return self.start_join();
                }
                match self.last_probe_at {
                    None => {
                        self.last_probe_at = Some(w.now);
                        Action::ProbeNow
                    }
                    Some(t) if w.now - t >= self.probe_interval => {
                        self.last_probe_at = Some(w.now);
                        Action::ProbeNow
                    }
                    _ => Action::None,
                }
            }
        }
    }

    fn start_join(&mut self) -> Action {
        self.phase = WPhase::Joining;
        self.auth_busy = false;
        self.next_auth_at = None;
        self.last_error = None;
        Action::Associate
    }

    fn reset_off(&mut self) {
        self.phase = WPhase::Off;
        self.auth_busy = false;
        self.next_auth_at = None;
        self.last_error = None;
        self.last_probe_at = None;
    }
}
