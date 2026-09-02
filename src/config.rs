use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const HEARTBEAT_MODULE_GDUT: &str = "gdut";

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Config {
    pub account: Account,
    pub dial: Dial,
    pub heartbeat: HeartbeatCfg,
    pub log: LogCfg,
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
        Ok(())
    }

    pub fn sample() -> String {
        let mut cfg = Config::default();
        cfg.account.student_id = "your_student_id".into();
        cfg.account.password_blob = String::new();
        format!(
            "# University Town auth server 10.0.3.2; Longdong/Dongfeng Road is 10.0.3.6\n# Heartbeat disabled by default; must verify via packet capture before enabling (see ADR-0002)\n{}",
            toml::to_string_pretty(&cfg).expect("default config is serializable")
        )
    }
}
