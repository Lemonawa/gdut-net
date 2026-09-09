//! `gdut-net wireless test`：现场验证 portal 常量（连 SSID → 等 IP → 临时 /32 →
//! 一次 login → 打印回包 → 自回滚断开）。见 spec §10。

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::adapter;
use crate::config::Config;
use crate::wireless::{portal, routes, wlan};

const WAIT_IP: Duration = Duration::from_secs(20);

pub fn cli_test(cfg_path: &Path) -> Result<()> {
    let cfg = Config::load(cfg_path)?;
    let pass = crate::crypto::unprotect(&cfg.account.password_blob)
        .context("Failed to decrypt password_blob (run install first)")?;
    let portal_ip = crate::probe::parse_http_probe_target(&cfg.wireless.portal_url)
        .map(|(ip, _)| ip)
        .context("wireless.portal_url invalid")?;

    println!("Joining SSID profile {:?} ...", cfg.wireless.profile);
    wlan::associate(&cfg.wireless.profile).context(
        "Failed to associate (profile missing? connect to the SSID manually once to create it)",
    )?;
    let deadline = Instant::now() + WAIT_IP;
    let adapter = loop {
        if let Some(a) = adapter::wlan_adapter() {
            break a;
        }
        if Instant::now() > deadline {
            bail!("Timed out waiting for WLAN IPv4 (DHCP)");
        }
        std::thread::sleep(Duration::from_secs(2));
    };
    let gw = adapter
        .gateway
        .context("WLAN has no gateway — portal unreachable")?;
    println!(
        "WLAN up: {} gw {} (if {})",
        adapter.ipv4, gw, adapter.ifindex
    );

    // 自包含 /32：结束即删（含失败路径——scope guard 手法）
    struct Teardown(routes::RouteGuard);
    impl Drop for Teardown {
        fn drop(&mut self) {
            self.0.teardown();
            let _ = wlan::disassociate();
        }
    }
    let mut guard = routes::RouteGuard::new();
    guard.ensure(&[portal_ip], gw, adapter.ifindex);
    let _teardown = Teardown(guard);

    let url = portal::build_login_url(
        &cfg.wireless.portal_url,
        &cfg.account.student_id,
        &pass,
        adapter.ipv4,
        &cfg.wireless.wlan_ac_ip,
    );
    println!("GET {}", portal::redact_query(&url));
    // portal_get 是 async（spawn_blocking 内部实现）；CLI 同步上下文一次性调用。
    let reply = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(portal::portal_get(adapter.ipv4, &url));
    match reply {
        Some((code, body)) => {
            println!("HTTP {code}");
            println!("Body: {body}");
            match portal::parse_portal_reply(&body) {
                portal::PortalResult::Success => println!("RESULT: SUCCESS"),
                portal::PortalResult::Failure(m) => println!("RESULT: FAILURE ({m})"),
                portal::PortalResult::Malformed => {
                    println!("RESULT: MALFORMED (check wlan_ac_ip / portal_url)")
                }
            }
        }
        None => println!("RESULT: NO REPLY (route/TUN interference? see ADR-0005 §routes)"),
    }
    println!("Cleaning up (disconnecting WLAN) ...");
    drop(_teardown);
    println!("Done.");
    Ok(())
}
