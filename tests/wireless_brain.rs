use gdut_net::ipc::protocol::{NetMode, WPhase};
use gdut_net::probe::ProbeVerdict;
use gdut_net::wireless::*;

fn brain() -> Brain {
    Brain::new(NetMode::WiredExclusive, 8, 10, 30)
}
fn world(now: u64, wired: bool, assoc: bool, ip: bool, probe: Option<ProbeVerdict>) -> World {
    World {
        now,
        eth_link_up: wired,
        wired_connected: wired,
        wlan_associated: assoc,
        wlan_ip: ip,
        probe,
    }
}

#[test]
fn exclusive_takeover_debounces_then_joins_and_auths() {
    let mut b = brain();
    assert_eq!(b.decide(&world(0, false, false, false, None)), Action::None);
    assert_eq!(b.decide(&world(7, false, false, false, None)), Action::None);
    assert_eq!(
        b.decide(&world(8, false, false, false, None)),
        Action::Associate
    );
    // 关联+IP 到位 → 立即认证
    assert_eq!(
        b.decide(&world(9, false, true, true, None)),
        Action::PortalAuth
    );
    b.on_auth(true, "ok", 9);
    assert_eq!(
        b.decide(&world(9, false, true, true, None)),
        Action::ProbeNow
    );
    assert_eq!(
        b.decide(&world(20, false, true, true, Some(ProbeVerdict::Alive))),
        Action::None
    );
    assert_eq!(
        b.decide(&world(38, false, true, true, Some(ProbeVerdict::Alive))),
        Action::None
    );
    assert_eq!(
        b.decide(&world(38 + 1, false, true, true, Some(ProbeVerdict::Alive))),
        Action::ProbeNow
    );
}

#[test]
fn exclusive_releases_after_wired_stable() {
    let mut b = brain();
    b.decide(&world(8, false, true, true, None)); // Associate → Online 流程走完
    b.decide(&world(9, false, true, true, None));
    b.on_auth(true, "ok", 9);
    assert_eq!(
        b.decide(&world(30, true, true, true, Some(ProbeVerdict::Alive))),
        Action::None
    );
    assert_eq!(
        b.decide(&world(39, true, true, true, Some(ProbeVerdict::Alive))),
        Action::None
    );
    assert_eq!(
        b.decide(&world(40, true, true, true, Some(ProbeVerdict::Alive))),
        Action::Disassociate
    );
    assert_eq!(b.decide(&world(41, true, false, false, None)), Action::None); // Off，有线健康不再动作
}

#[test]
fn standby_ignores_wired_state() {
    let mut b = Brain::new(NetMode::WiredPlusStandby, 8, 10, 30);
    assert_eq!(
        b.decide(&world(0, true, false, false, None)),
        Action::Associate
    );
    assert_eq!(
        b.decide(&world(1, true, true, true, None)),
        Action::PortalAuth
    );
}

#[test]
fn kicked_reauths_immediately() {
    let mut b = brain();
    b.decide(&world(8, false, true, true, None));
    b.decide(&world(9, false, true, true, None)); // PortalAuth
    b.on_auth(true, "ok", 9);
    b.decide(&world(9, false, true, true, None)); // ProbeNow
    assert_eq!(
        b.decide(&world(10, false, true, true, Some(ProbeVerdict::Kicked))),
        Action::PortalAuth
    );
}

/// 回归（T9 review Critical）：Kicked 触发重认证成功后，manager 必须以
/// probe=None 供下一拍决策——Brain 侧契约是此时返回 ProbeNow 而非再次
/// PortalAuth。若残留 Kicked（Online 相 Kicked 检查先于探测定时器），
/// manager 会陷入 ~2s 一次的无限重认证循环。
#[test]
fn kicked_verdict_must_be_consumed_after_reauth() {
    let mut b = brain();
    b.decide(&world(8, false, true, true, None)); // Associate
    b.decide(&world(9, false, true, true, None)); // PortalAuth
    b.on_auth(true, "ok", 9); // Online
    b.decide(&world(9, false, true, true, None)); // ProbeNow
    assert_eq!(
        b.decide(&world(10, false, true, true, Some(ProbeVerdict::Kicked))),
        Action::PortalAuth
    );
    // Re-auth succeeds: Online again, probe timer reset.
    b.on_auth(true, "re-login ok", 10);
    // Manager cleared the cached verdict (probe=None), so the Brain must
    // demand a fresh probe here -- a stale Kicked would re-trigger PortalAuth
    // and pin the manager in a ~2s re-auth loop (review Critical).
    assert_eq!(
        b.decide(&world(10, false, true, true, None)),
        Action::ProbeNow
    );
    // 收到 Alive 后回归稳态：不再认证、按定时器探测。
    assert_eq!(
        b.decide(&world(11, false, true, true, Some(ProbeVerdict::Alive))),
        Action::None
    );
}

#[test]
fn auth_failure_backs_off_5_15_30() {
    let mut b = brain();
    b.decide(&world(8, false, true, true, None));
    b.decide(&world(9, false, true, true, None)); // PortalAuth, phase Authing
    b.on_auth(false, "result=0", 9);
    assert_eq!(b.phase(), WPhase::Error);
    assert_eq!(b.decide(&world(13, false, true, true, None)), Action::None);
    assert_eq!(
        b.decide(&world(14, false, true, true, None)),
        Action::Associate
    ); // 重走 join→auth
    b.on_auth(false, "again", 15);
    b.on_auth(false, "third", 16); // 第 3 次失败 → 30s
    assert_eq!(b.decide(&world(45, false, true, true, None)), Action::None);
    assert_eq!(
        b.decide(&world(46, false, true, true, None)),
        Action::Associate
    );
}

#[test]
fn assoc_lost_rejoins_and_mode_switch_releases() {
    let mut b = brain();
    b.decide(&world(8, false, true, true, None));
    b.decide(&world(9, false, true, true, None));
    b.on_auth(true, "ok", 9);
    assert_eq!(
        b.decide(&world(30, false, false, false, None)),
        Action::Associate
    );
    // standby→exclusive 且有线健康：10s 后让位
    b.set_mode(NetMode::WiredExclusive);
    b.on_auth(true, "ok", 31);
    assert_eq!(
        b.decide(&world(100, true, true, true, Some(ProbeVerdict::Alive))),
        Action::None
    );
    assert_eq!(
        b.decide(&world(110, true, true, true, Some(ProbeVerdict::Alive))),
        Action::Disassociate
    );
}
