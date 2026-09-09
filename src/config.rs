use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::ipc::protocol::NetMode;

pub const HEARTBEAT_MODULE_GDUT: &str = "gdut";

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Config {
    pub account: Account,
    pub dial: Dial,
    pub heartbeat: HeartbeatCfg,
    pub log: LogCfg,
    #[serde(default)]
    pub wireless: WirelessCfg,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Account {
    pub student_id: String,
    pub password_blob: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Dial {
    pub entry_name: String,
    pub pbk_path: String,
    /// 指定物理适配器 FriendlyName；空串=自动选择。
    #[serde(default)]
    pub interface: String,
    pub probe_interval_secs: u64,
    pub http_probe_url: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HeartbeatCfg {
    pub enabled: bool,
    pub module: String,
    pub server: String,
    pub port: u16,
    pub interval_secs: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LogCfg {
    pub dir: String,
    pub max_size_mb: u64,
    pub rotate_keep: u32,
    pub event_log: bool,
}

/// 无线接管配置（ADR-0005）：ssid 与 profile 同值且 WlanConnect 只用 profile，砍掉冗余字段。
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WirelessCfg {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub mode: NetMode,
    /// Windows WLAN profile 名（netsh wlan connect 用）。
    #[serde(default = "default_wlan_profile")]
    pub profile: String,
    /// HEMC eportal 登录接口。
    #[serde(default = "default_portal_url")]
    pub portal_url: String,
    /// HEMC AC 地址。
    #[serde(default = "default_wlan_ac_ip")]
    pub wlan_ac_ip: String,
    #[serde(default = "default_probe_host")]
    pub probe_host: String,
    /// 失联去抖：连续失联该秒数后才接管。
    #[serde(default = "default_takeover_after")]
    pub takeover_after_secs: u64,
    /// 恢复去抖：有线稳定该秒数后让位。
    #[serde(default = "default_release_after")]
    pub release_after_secs: u64,
    /// standby 模式下 WLAN 路由 metric 压制值；0 = 不压制。
    #[serde(default = "default_standby_metric")]
    pub standby_metric: u32,
}

impl Default for Dial {
    fn default() -> Self {
        Self {
            entry_name: "gdut".into(),
            pbk_path: r"C:\ProgramData\gdut-net\gdut.pbk".into(),
            interface: String::new(),
            probe_interval_secs: 30,
            http_probe_url: "http://223.5.5.5".into(),
        }
    }
}

impl Default for HeartbeatCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            module: HEARTBEAT_MODULE_GDUT.into(),
            server: "10.0.3.2".into(),
            port: 61440,
            interval_secs: 20,
        }
    }
}

impl Default for LogCfg {
    fn default() -> Self {
        Self {
            dir: r"C:\ProgramData\gdut-net\logs".into(),
            max_size_mb: 5,
            rotate_keep: 5,
            event_log: false,
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_wlan_profile() -> String {
    "gdut".into()
}
fn default_portal_url() -> String {
    "http://10.0.3.2:801/eportal/portal/login".into()
}
fn default_wlan_ac_ip() -> String {
    "172.16.254.2".into()
}
fn default_probe_host() -> String {
    "223.5.5.5".into()
}
fn default_takeover_after() -> u64 {
    8
}
fn default_release_after() -> u64 {
    10
}
fn default_standby_metric() -> u32 {
    10
}

impl Default for WirelessCfg {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            mode: NetMode::default(),
            profile: default_wlan_profile(),
            portal_url: default_portal_url(),
            wlan_ac_ip: default_wlan_ac_ip(),
            probe_host: default_probe_host(),
            takeover_after_secs: default_takeover_after(),
            release_after_secs: default_release_after(),
            standby_metric: default_standby_metric(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        let cfg: Config = toml::from_str(&raw)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
            }
        }
        let body = toml::to_string_pretty(self).context("Failed to serialize config")?;
        fs::write(path, body)
            .with_context(|| format!("Failed to write config file: {}", path.display()))?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if self.account.student_id.trim().is_empty() {
            anyhow::bail!("account.student_id must not be empty");
        }
        if self.heartbeat.enabled && self.heartbeat.module != HEARTBEAT_MODULE_GDUT {
            anyhow::bail!(
                "heartbeat.module only supports \"{}\", got {:?}",
                HEARTBEAT_MODULE_GDUT,
                self.heartbeat.module
            );
        }
        if self.dial.probe_interval_secs < 5 {
            anyhow::bail!("dial.probe_interval_secs must be >= 5");
        }
        if crate::probe::parse_http_probe_target(&self.dial.http_probe_url).is_none() {
            anyhow::bail!(
                "dial.http_probe_url must be http:// + IPv4 literal (with optional port), got {:?}",
                self.dial.http_probe_url
            );
        }
        let w = &self.wireless;
        if w.enabled {
            if crate::probe::parse_http_probe_target(&w.portal_url).is_none() {
                anyhow::bail!(
                    "wireless.portal_url must be http:// + IPv4 literal (with optional port), got {:?}",
                    w.portal_url
                );
            }
            if w.probe_host.parse::<std::net::Ipv4Addr>().is_err() {
                anyhow::bail!(
                    "wireless.probe_host must be an IPv4 literal, got {:?}",
                    w.probe_host
                );
            }
            if w.takeover_after_secs < 1 || w.release_after_secs < 1 {
                anyhow::bail!("wireless takeover_after_secs/release_after_secs must be >= 1");
            }
            if w.standby_metric > 9999 {
                anyhow::bail!("wireless.standby_metric must be <= 9999 (0 = disable)");
            }
        }
        Ok(())
    }

    pub fn sample() -> String {
        let mut cfg = Config::default();
        cfg.account.student_id = "your_student_id".into();
        cfg.account.password_blob = String::new();
        format!(
            "# HEMC (Higher Education Mega Center) auth server 10.0.3.2; Longdong/Dongfeng Road is 10.0.3.6\n# Heartbeat disabled by default; must verify via packet capture before enabling (see ADR-0002)\n{}",
            toml::to_string_pretty(&cfg).expect("default config is serializable")
        )
    }
}
