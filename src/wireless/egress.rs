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
    fn add_route(&mut self, dest: Ipv4Addr, gateway: Ipv4Addr, ifindex: u32) -> anyhow::Result<()>;
    fn delete_route(
        &mut self,
        dest: Ipv4Addr,
        gateway: Ipv4Addr,
        ifindex: u32,
    ) -> anyhow::Result<()>;
    fn suppress_metric(&mut self, ifindex: u32, target: u32);
    fn restore_metric(&mut self, ifindex: u32) -> anyhow::Result<()>;
}

#[derive(Default)]
pub struct WirelessEgress<T: EgressOps> {
    ops: T,
    dests: Vec<Ipv4Addr>,
    added: Vec<(Ipv4Addr, Ipv4Addr, u32)>,
    /// 待下发/需迁移的目标集合；metric 幂等仍由 Windows adapter 保存原值。
    applied_endpoint: Option<WlanEndpoint>,
    route_endpoint: Option<WlanEndpoint>,
    metric_release_pending: bool,
}

impl<T: EgressOps> WirelessEgress<T> {
    pub fn new(ops: T) -> Self {
        Self {
            ops,
            dests: Vec::new(),
            added: Vec::new(),
            applied_endpoint: None,
            route_endpoint: None,
            metric_release_pending: false,
        }
    }

    /// 声明本服务拥有的 /32 目标（启动后只会随配置变更，运行期保持不变）。
    pub fn own_destinations(&mut self, dests: &[Ipv4Addr]) {
        self.dests = dests.to_vec();
        if let Err(e) = self.retry_cleanup() {
            log::warn!("Wireless Egress destination cleanup failed (kept for retry): {e:#}");
        }
    }

    pub fn has_pending_cleanup(&self) -> bool {
        self.metric_release_pending
            || self.added.iter().any(|&(dest, gateway, ifindex)| {
                !self.dests.contains(&dest)
                    || self.route_endpoint != Some(WlanEndpoint { gateway, ifindex })
            })
    }

    /// 重试已让位/迁移的路由；执行器处理消息和空闲定时器时调用。
    pub fn retry_cleanup(&mut self) -> anyhow::Result<()> {
        let mut failure = None;
        self.added.retain(|&(dest, gateway, ifindex)| {
            let wanted = self.dests.contains(&dest)
                && self.route_endpoint == Some(WlanEndpoint { gateway, ifindex });
            if wanted {
                return true;
            }
            match self.ops.delete_route(dest, gateway, ifindex) {
                Ok(()) => false,
                Err(e) => {
                    failure = Some(e);
                    true
                }
            }
        });
        if self.metric_release_pending {
            if let Err(e) = self.release_metric() {
                failure = Some(e);
            }
        }
        match failure {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// 获取出口；失败保留未清理的账本，下一次认证可重试。
    pub fn acquire(&mut self, endpoint: WlanEndpoint) -> anyhow::Result<Duration> {
        self.route_endpoint = Some(endpoint);
        let mut failure = self.retry_cleanup().err();
        for dest in &self.dests {
            let want = (*dest, endpoint.gateway, endpoint.ifindex);
            if !self.added.contains(&want) {
                match self
                    .ops
                    .add_route(*dest, endpoint.gateway, endpoint.ifindex)
                {
                    Ok(()) => self.added.push(want),
                    Err(e) => failure = Some(e),
                }
            }
        }
        self.applied_endpoint = Some(endpoint);
        match failure {
            Some(e) => Err(e),
            None => Ok(ROUTE_SETTLE),
        }
    }

    pub fn wait_ready(&self) -> Duration {
        ROUTE_SETTLE
    }

    pub fn suppress_metric(&mut self, endpoint: WlanEndpoint, target: u32) {
        if target == 0 {
            return;
        }
        self.metric_release_pending = false;
        self.ops.suppress_metric(endpoint.ifindex, target);
        self.applied_endpoint = Some(endpoint);
    }

    pub fn release_metric(&mut self) -> anyhow::Result<()> {
        let Some(endpoint) = self.applied_endpoint else {
            return Ok(());
        };
        self.metric_release_pending = true;
        self.ops.restore_metric(endpoint.ifindex)?;
        self.metric_release_pending = false;
        Ok(())
    }

    /// 全量释放：/32 + metric。释放后模块可复用。
    pub fn release(&mut self) -> anyhow::Result<()> {
        self.route_endpoint = None;
        let routes = self.retry_cleanup();
        let metric = self.release_metric();
        if metric.is_ok() {
            self.applied_endpoint = None;
        }
        routes.and(metric)
    }
}

/// 防御纵深：panic/unwind 亦释放；显式 release 后为空操作。
impl<T: EgressOps> Drop for WirelessEgress<T> {
    fn drop(&mut self) {
        if let Err(e) = self.release() {
            log::warn!("Wireless Egress cleanup failed (adapter will retry on drop): {e:#}");
        }
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
        fail_add: bool,
        fail_delete: bool,
        fail_restore: bool,
    }

    struct ScriptedOps {
        ledger: Rc<RefCell<Ledger>>,
    }

    impl EgressOps for ScriptedOps {
        fn add_route(
            &mut self,
            dest: Ipv4Addr,
            gateway: Ipv4Addr,
            ifindex: u32,
        ) -> anyhow::Result<()> {
            anyhow::ensure!(!self.ledger.borrow().fail_add, "injected add failure");
            self.ledger
                .borrow_mut()
                .routes
                .push((dest, gateway, ifindex));
            Ok(())
        }
        fn delete_route(
            &mut self,
            dest: Ipv4Addr,
            gateway: Ipv4Addr,
            ifindex: u32,
        ) -> anyhow::Result<()> {
            anyhow::ensure!(!self.ledger.borrow().fail_delete, "injected delete failure");
            self.ledger
                .borrow_mut()
                .routes
                .retain(|row| *row != (dest, gateway, ifindex));
            Ok(())
        }
        fn suppress_metric(&mut self, ifindex: u32, metric: u32) {
            self.ledger
                .borrow_mut()
                .metrics
                .push(MetricChange { ifindex, metric });
        }
        fn restore_metric(&mut self, ifindex: u32) -> anyhow::Result<()> {
            anyhow::ensure!(
                !self.ledger.borrow().fail_restore,
                "injected metric restore failure"
            );
            self.ledger.borrow_mut().released.push(ifindex);
            Ok(())
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
        assert_eq!(egress.acquire(endpoint(15)).unwrap(), ROUTE_SETTLE);
        assert_eq!(
            ledger.borrow().routes,
            vec![(ip(2), ip(1), 15), (ip(3), ip(1), 15)]
        );
        assert_eq!(egress.wait_ready(), ROUTE_SETTLE);
    }

    #[test]
    fn reacquire_migrates_only_changed_tuples() {
        let (mut egress, ledger) = egress();
        egress.acquire(endpoint(15)).unwrap();
        egress.acquire(endpoint(16)).unwrap();
        assert_eq!(
            ledger.borrow().routes,
            vec![(ip(2), ip(1), 16), (ip(3), ip(1), 16)]
        );
    }

    #[test]
    fn release_deletes_routes_and_restores_metric_once() {
        let (mut egress, ledger) = egress();
        egress.acquire(endpoint(15)).unwrap();
        egress.suppress_metric(endpoint(15), 100);
        egress.release().unwrap();
        egress.release().unwrap();
        assert!(ledger.borrow().routes.is_empty());
        assert_eq!(ledger.borrow().released, vec![15]);
    }

    #[test]
    fn drop_releases_routes_after_scope_exit() {
        let (mut egress, ledger) = egress();
        egress.acquire(endpoint(15)).unwrap();
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
        egress.release().unwrap();
        assert!(ledger.borrow().released.is_empty());
    }
    #[test]
    fn failed_add_is_retried_at_the_same_endpoint() {
        let (mut egress, ledger) = egress();
        ledger.borrow_mut().fail_add = true;
        assert!(egress.acquire(endpoint(15)).is_err());
        assert!(ledger.borrow().routes.is_empty());
        ledger.borrow_mut().fail_add = false;
        egress.acquire(endpoint(15)).unwrap();
        assert_eq!(ledger.borrow().routes.len(), 2);
    }

    #[test]
    fn failed_release_keeps_routes_for_retry() {
        let (mut egress, ledger) = egress();
        egress.acquire(endpoint(15)).unwrap();
        ledger.borrow_mut().fail_delete = true;
        assert!(egress.release().is_err());
        assert_eq!(ledger.borrow().routes.len(), 2);
        ledger.borrow_mut().fail_delete = false;
        egress.release().unwrap();
        assert!(ledger.borrow().routes.is_empty());
    }

    #[test]
    fn failed_migration_delete_is_retried_even_when_new_route_exists() {
        let (mut egress, ledger) = egress();
        egress.acquire(endpoint(15)).unwrap();
        ledger.borrow_mut().fail_delete = true;
        assert!(egress.acquire(endpoint(16)).is_err());
        assert_eq!(ledger.borrow().routes.len(), 4);
        ledger.borrow_mut().fail_delete = false;
        egress.acquire(endpoint(16)).unwrap();
        assert_eq!(
            ledger.borrow().routes,
            vec![(ip(2), ip(1), 16), (ip(3), ip(1), 16)]
        );
    }

    #[test]
    fn drop_retries_a_failed_explicit_release() {
        let (mut egress, ledger) = egress();
        egress.acquire(endpoint(15)).unwrap();
        ledger.borrow_mut().fail_delete = true;
        assert!(egress.release().is_err());
        ledger.borrow_mut().fail_delete = false;
        drop(egress);
        assert!(ledger.borrow().routes.is_empty());
    }
    #[test]
    fn periodic_cleanup_retries_release_without_authenticating_again() {
        let (mut egress, ledger) = egress();
        egress.acquire(endpoint(15)).unwrap();
        ledger.borrow_mut().fail_delete = true;
        assert!(egress.release().is_err());
        assert!(egress.has_pending_cleanup());
        ledger.borrow_mut().fail_delete = false;
        egress.retry_cleanup().unwrap();
        assert!(!egress.has_pending_cleanup());
        assert!(ledger.borrow().routes.is_empty());
    }

    #[test]
    fn reacquiring_endpoint_adopts_routes_left_by_failed_release() {
        let (mut egress, ledger) = egress();
        egress.acquire(endpoint(15)).unwrap();
        ledger.borrow_mut().fail_delete = true;
        assert!(egress.release().is_err());
        egress.acquire(endpoint(15)).unwrap();
        assert_eq!(ledger.borrow().routes.len(), 2);
        ledger.borrow_mut().fail_delete = false;
        egress.retry_cleanup().unwrap();
        assert_eq!(ledger.borrow().routes.len(), 2);
    }
    #[test]
    fn periodic_cleanup_retries_metric_restore_after_failed_release() {
        let (mut egress, ledger) = egress();
        egress.suppress_metric(endpoint(15), 100);
        ledger.borrow_mut().fail_restore = true;
        assert!(egress.release().is_err());
        assert!(egress.has_pending_cleanup());
        ledger.borrow_mut().fail_restore = false;
        egress.retry_cleanup().unwrap();
        assert!(!egress.has_pending_cleanup());
        assert_eq!(ledger.borrow().released, vec![15]);
    }
}
