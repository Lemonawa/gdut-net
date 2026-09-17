//! `gdut-net wireless test`：现场验证 portal 常量（连 SSID → 等 IP → 临时 /32 →
//! 一次 login → 打印回包 → 自回滚断开）。见 spec §10。

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::adapter;
use crate::config::Config;
use crate::wireless::{egress, portal, routes, wlan};

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

    // 自包含 Wireless Egress：结束即释放（含失败路径）。
    let mut wireless_egress = egress::WirelessEgress::new(routes::RouteGuard::new());
    wireless_egress.own_destinations(&[portal_ip]);
    let settle = wireless_egress.acquire(egress::WlanEndpoint {
        gateway: gw,
        ifindex: adapter.ifindex,
    });
    struct Teardown(egress::WirelessEgress<routes::RouteGuard>);
    impl Drop for Teardown {
        fn drop(&mut self) {
            self.0.release();
            let _ = wlan::disassociate();
        }
    }
    let _teardown = Teardown(wireless_egress);
    println!("Waiting {}ms for route propagation ...", settle.as_millis());
    std::thread::sleep(settle);

    let url = portal::build_login_url(
        &cfg.wireless.portal_url,
        &cfg.account.student_id,
        &pass,
        adapter.ipv4,
        &cfg.wireless.wlan_ac_ip,
    );
    println!("GET {}", portal::redact_query(&url));
    // 绑源 SYN 偶发被丢（diag 2026-09-10）：单次尝试不可靠，连试三次。
    let mut reply = None;
    for attempt in 1..=3 {
        let r = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(portal::portal_get(adapter.ipv4, &url));
        if r.is_some() {
            reply = r;
            break;
        }
        println!("No reply (attempt {attempt}/3), retrying ...");
        std::thread::sleep(Duration::from_secs(2));
    }
    match reply {
        Some((code, body)) => {
            println!("HTTP {code}");
            println!("Body: {body}");
            match portal::parse_portal_reply(&body) {
                portal::PortalResult::Success => println!("RESULT: SUCCESS"),
                portal::PortalResult::AlreadyOnline => {
                    println!("RESULT: SUCCESS (already online)")
                }
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
