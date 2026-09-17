//! 纯逻辑 Wireless Egress：管理 /32 路由账本、metric 压制和等待窗口。
//!
//! 该模块只做决策与记账；Windows `routes.rs` 是它的 adapter。Supervisor 请求
//! `acquire/wait_ready/release`，不直接拼装 `EnsureRoutes + Settle` 等机械序列。

use std::net::Ipv4Addr;
use std::time::Duration;

/// /32 写入后的数据面生效等待（真机实测 2–8s 竞态窗口）。
pub const ROUTE_SETTLE: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WlanEndpoint {
    pub gateway: Ipv4Addr,
    pub ifindex: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteChange {
    Add {
        dest: Ipv4Addr,
        gateway: Ipv4Addr,
        ifindex: u32,
    },
    Delete {
        dest: Ipv4Addr,
        gateway: Ipv4Addr,
        ifindex: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetricChange {
    pub ifindex: u32,
    pub metric: u32,
}

/// Wireless Egress 的可测试 adapter seam。
pub trait EgressOps {
    fn add_route(&mut self, dest: Ipv4Addr, gateway: Ipv4Addr, ifindex: u32);
    fn delete_route(&mut self, dest: Ipv4Addr, gateway: Ipv4Addr, ifindex: u32);
    fn suppress_metric(&mut self, ifindex: u32, target: u32);
    fn restore_metric(&mut self, ifindex: u32);
}

#[derive(Default)]
pub struct WirelessEgress<T: EgressOps> {
    ops: T,
    dests: Vec<Ipv4Addr>,
    added: Vec<(Ipv4Addr, Ipv4Addr, u32)>,
    /// 待下发/需迁移的目标集合；metric 幂等仍由 Windows adapter 保存原值。
    applied_endpoint: Option<WlanEndpoint>,
}

impl<T: EgressOps> WirelessEgress<T> {
    pub fn new(ops: T) -> Self {
        Self {
            ops,
            dests: Vec::new(),
            added: Vec::new(),
            applied_endpoint: None,
        }
    }

    /// 声明本服务拥有的 /32 目标（启动后只会随配置变更，运行期保持不变）。
    pub fn own_destinations(&mut self, dests: &[Ipv4Addr]) {
        self.dests = dests.to_vec();
        self.added.retain(|(dest, gateway, ifindex)| {
            if dests.contains(dest) {
                true
            } else {
                self.ops.delete_route(*dest, *gateway, *ifindex);
                false
            }
        });
    }

    /// 获取 Wireless Egress：删旧三元组、按新网关/ifindex 加 /32，返回等待窗口。
    pub fn acquire(&mut self, endpoint: WlanEndpoint) -> Duration {
        for dest in &self.dests {
            let want = (*dest, endpoint.gateway, endpoint.ifindex);
            if self.added.contains(&want) {
                continue;
            }
            self.added.retain(|(old, gateway, ifindex)| {
                if *old == *dest {
                    self.ops.delete_route(*old, *gateway, *ifindex);
                    false
                } else {
                    true
                }
            });
            self.ops
                .add_route(*dest, endpoint.gateway, endpoint.ifindex);
            self.added.push(want);
        }
        self.applied_endpoint = Some(endpoint);
        ROUTE_SETTLE
    }

    pub fn wait_ready(&self) -> Duration {
        ROUTE_SETTLE
    }

    pub fn suppress_metric(&mut self, endpoint: WlanEndpoint, target: u32) {
        if target == 0 {
            return;
        }
        self.ops.suppress_metric(endpoint.ifindex, target);
        self.applied_endpoint = Some(endpoint);
    }

    pub fn release_metric(&mut self) {
        let Some(endpoint) = self.applied_endpoint else {
            return;
        };
        self.ops.restore_metric(endpoint.ifindex);
    }

    /// 全量释放：/32 + metric。释放后模块可复用。
    pub fn release(&mut self) {
        for (dest, gateway, ifindex) in self.added.drain(..) {
            self.ops.delete_route(dest, gateway, ifindex);
        }
        self.release_metric();
        self.applied_endpoint = None;
    }
}

/// 防御纵深：panic/unwind 亦释放；显式 release 后为空操作。
impl<T: EgressOps> Drop for WirelessEgress<T> {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct Ledger {
        routes: Vec<(Ipv4Addr, Ipv4Addr, u32)>,
        metrics: Vec<MetricChange>,
        released: Vec<u32>,
    }

    struct ScriptedOps {
        ledger: Rc<RefCell<Ledger>>,
    }

    impl EgressOps for ScriptedOps {
        fn add_route(&mut self, dest: Ipv4Addr, gateway: Ipv4Addr, ifindex: u32) {
            self.ledger
                .borrow_mut()
                .routes
                .push((dest, gateway, ifindex));
        }
        fn delete_route(&mut self, dest: Ipv4Addr, gateway: Ipv4Addr, ifindex: u32) {
            self.ledger
                .borrow_mut()
                .routes
                .retain(|row| *row != (dest, gateway, ifindex));
        }
        fn suppress_metric(&mut self, ifindex: u32, metric: u32) {
            self.ledger
                .borrow_mut()
                .metrics
                .push(MetricChange { ifindex, metric });
        }
        fn restore_metric(&mut self, ifindex: u32) {
            self.ledger.borrow_mut().released.push(ifindex);
        }
    }

    fn ip(n: u8) -> Ipv4Addr {
        Ipv4Addr::new(10, 0, 3, n)
    }

    fn endpoint(ifindex: u32) -> WlanEndpoint {
        WlanEndpoint {
            gateway: ip(1),
            ifindex,
        }
    }

    fn egress() -> (WirelessEgress<ScriptedOps>, Rc<RefCell<Ledger>>) {
        let ledger = Rc::new(RefCell::new(Ledger::default()));
        let mut egress = WirelessEgress::new(ScriptedOps {
            ledger: ledger.clone(),
        });
        egress.own_destinations(&[ip(2), ip(3)]);
        (egress, ledger)
    }

    #[test]
    fn acquire_adds_owned_destinations_and_returns_ready_wait() {
        let (mut egress, ledger) = egress();
        assert_eq!(egress.acquire(endpoint(15)), ROUTE_SETTLE);
        assert_eq!(
            ledger.borrow().routes,
            vec![(ip(2), ip(1), 15), (ip(3), ip(1), 15)]
        );
        assert_eq!(egress.wait_ready(), ROUTE_SETTLE);
    }

    #[test]
    fn reacquire_migrates_only_changed_tuples() {
        let (mut egress, ledger) = egress();
        egress.acquire(endpoint(15));
        egress.acquire(endpoint(16));
        assert_eq!(
            ledger.borrow().routes,
            vec![(ip(2), ip(1), 16), (ip(3), ip(1), 16)]
        );
    }

    #[test]
    fn release_deletes_routes_and_restores_metric_once() {
        let (mut egress, ledger) = egress();
        egress.acquire(endpoint(15));
        egress.suppress_metric(endpoint(15), 100);
        egress.release();
        egress.release();
        assert!(ledger.borrow().routes.is_empty());
        assert_eq!(ledger.borrow().released, vec![15]);
    }

    #[test]
    fn drop_releases_routes_after_scope_exit() {
        let (mut egress, ledger) = egress();
        egress.acquire(endpoint(15));
        {
            let _owned = egress;
        }
        assert!(ledger.borrow().routes.is_empty());
    }

    #[test]
    fn zero_metric_remains_unmanaged() {
        let (mut egress, ledger) = egress();
        egress.suppress_metric(endpoint(15), 0);
        assert!(ledger.borrow().metrics.is_empty());
        egress.release();
        assert!(ledger.borrow().released.is_empty());
    }
}
