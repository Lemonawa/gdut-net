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
            .with_context(|| format!("读取配置文件失败: {}", path.display()))?;
        let cfg: Config = toml::from_str(&raw)
            .with_context(|| format!("解析配置文件失败: {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("创建目录失败: {}", parent.display()))?;
            }
        }
        let body = toml::to_string_pretty(self).context("序列化配置失败")?;
        fs::write(path, body).with_context(|| format!("写入配置文件失败: {}", path.display()))?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if self.account.student_id.trim().is_empty() {
            anyhow::bail!("account.student_id 不能为空");
        }
        if self.heartbeat.enabled && self.heartbeat.module != HEARTBEAT_MODULE_GDUT {
            anyhow::bail!(
                "heartbeat.module 只支持 \"{}\"，当前为 {:?}",
                HEARTBEAT_MODULE_GDUT,
                self.heartbeat.module
            );
        }
        if self.dial.probe_interval_secs < 5 {
            anyhow::bail!("dial.probe_interval_secs 不得小于 5 秒");
        }
        if crate::probe::parse_http_probe_target(&self.dial.http_probe_url).is_none() {
            anyhow::bail!(
                "dial.http_probe_url 必须是 http:// 加 IPv4 字面量（可带端口），当前为 {:?}",
                self.dial.http_probe_url
            );
        }
        Ok(())
    }

    pub fn sample() -> String {
        let mut cfg = Config::default();
        cfg.account.student_id = "你的学号".into();
        cfg.account.password_blob = String::new();
        format!(
            "# 大学城认证服务器 10.0.3.2；龙洞/东风路为 10.0.3.6\n# 心跳默认关闭；开启前必须抓包验证（见 ADR-0002）\n{}",
            toml::to_string_pretty(&cfg).expect("默认配置可序列化")
        )
    }
}
