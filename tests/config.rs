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

// Brief deviation: Config::default() derives an empty account.student_id which
// validate() rejects, so seed it to reach the wireless checks under test.
#[test]
fn wireless_defaults_and_validation() {
    let mut cfg = Config::default();
    cfg.account.student_id = "202100000000".into();
    assert!(cfg.wireless.enabled);
    assert_eq!(cfg.wireless.profile, "gdut");
    // 真机实测调参（final review）：100 保有线主路由、压过被墙物理口。
    assert_eq!(cfg.wireless.standby_metric, 100);
    assert!(cfg.validate().is_ok());

    cfg.wireless.probe_host = "not-an-ip".into();
    assert!(cfg.validate().is_err());

    cfg.wireless.probe_host = "223.5.5.5".into();
    cfg.wireless.portal_url = "http://portal.example.com/login".into(); // invalid: domain, not IPv4 literal
    assert!(cfg.validate().is_err());

    cfg.wireless.portal_url = "http://10.0.3.2:801/eportal/portal/login".into();
    cfg.wireless.takeover_after_secs = 0;
    assert!(cfg.validate().is_err());

    cfg.wireless.takeover_after_secs = 8;
    assert!(cfg.validate().is_ok());
}

#[test]
fn sample_round_trips_with_wireless() {
    let cfg: Config = toml::from_str(&Config::sample()).unwrap();
    assert!(cfg.validate().is_ok());
    assert_eq!(cfg.wireless.wlan_ac_ip, "172.16.254.2");
}
