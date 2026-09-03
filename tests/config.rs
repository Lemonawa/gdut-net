use gdut_net::config::Config;

#[test]
fn roundtrip_and_defaults() {
    let cfg: Config = toml::from_str(&Config::sample()).unwrap();
    cfg.validate().unwrap();
    assert_eq!(cfg.dial.entry_name, "gdut");
    assert!(!cfg.heartbeat.enabled);
    assert_eq!(cfg.heartbeat.server, "10.0.3.2");
    assert_eq!(cfg.heartbeat.port, 61440);
    assert_eq!(cfg.dial.probe_interval_secs, 30);
}

#[test]
fn reject_bad_heartbeat_module() {
    let mut cfg: Config = toml::from_str(&Config::sample()).unwrap();
    cfg.heartbeat.enabled = true;
    cfg.heartbeat.module = "unknown".into();
    assert!(cfg.validate().is_err());
}

#[test]
fn reject_short_probe_interval() {
    let mut cfg: Config = toml::from_str(&Config::sample()).unwrap();
    cfg.dial.probe_interval_secs = 1;
    assert!(cfg.validate().is_err());
}

#[test]
fn accept_default_http_probe_url_and_ipv4_with_port() {
    let cfg: Config = toml::from_str(&Config::sample()).unwrap();
    cfg.validate().unwrap();
    let mut cfg: Config = toml::from_str(&Config::sample()).unwrap();
    cfg.dial.http_probe_url = "http://192.168.191.1:8081/".into();
    cfg.validate().unwrap();
}

#[test]
fn reject_non_ipv4_http_probe_url() {
    let mut cfg: Config = toml::from_str(&Config::sample()).unwrap();
    cfg.dial.http_probe_url = "http://www.gdut.edu.cn".into();
    assert!(cfg.validate().is_err());
    let mut cfg: Config = toml::from_str(&Config::sample()).unwrap();
    cfg.dial.http_probe_url = "https://9.9.9.9".into();
    assert!(cfg.validate().is_err());
}
