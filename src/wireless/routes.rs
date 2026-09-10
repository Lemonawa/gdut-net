//! /32 主机路由 + WLAN 接口 metric 管理（ADR-0005）。
//! 自包含铁律：ensure/teardown 成对，三条出口（让位/切模式/服务停止）全覆盖。

use std::net::Ipv4Addr;

use anyhow::{anyhow, Result};
use windows::Win32::Foundation::{ERROR_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::NetworkManagement::IpHelper::{
    CreateIpForwardEntry2, DeleteIpForwardEntry2, GetIpForwardTable2, GetIpInterfaceEntry,
    InitializeIpForwardEntry, InitializeIpInterfaceEntry, SetIpInterfaceEntry, IP_ADDRESS_PREFIX,
    MIB_IPFORWARD_ROW2, MIB_IPFORWARD_TABLE2, MIB_IPINTERFACE_ROW,
};
use windows::Win32::Networking::WinSock::{
    AF_INET, IN_ADDR, IN_ADDR_0, MIB_IPPROTO_NETMGMT, SOCKADDR_IN, SOCKADDR_INET,
};

type Entry = (Ipv4Addr, Ipv4Addr, u32); // (dest, gw, ifindex)

fn sockaddr_in4(ip: Ipv4Addr) -> SOCKADDR_INET {
    SOCKADDR_INET {
        Ipv4: SOCKADDR_IN {
            sin_family: AF_INET,
            sin_addr: IN_ADDR {
                S_un: IN_ADDR_0 {
                    // 必须网络序（见 wireless::ipv4_to_s_addr 的教训注释）。
                    S_addr: crate::wireless::ipv4_to_s_addr(ip),
                },
            },
            ..Default::default()
        },
    }
}

fn forward_row(dest: Ipv4Addr, gw: Ipv4Addr, ifindex: u32) -> MIB_IPFORWARD_ROW2 {
    let mut row = MIB_IPFORWARD_ROW2::default();
    unsafe { InitializeIpForwardEntry(&mut row) };
    row.InterfaceIndex = ifindex;
    row.DestinationPrefix = IP_ADDRESS_PREFIX {
        Prefix: sockaddr_in4(dest),
        PrefixLength: 32,
    };
    row.NextHop = sockaddr_in4(gw);
    row.Metric = 1;
    row.Protocol = MIB_IPPROTO_NETMGMT;
    row
}

fn add(dest: Ipv4Addr, gw: Ipv4Addr, ifindex: u32) -> Result<()> {
    let row = forward_row(dest, gw, ifindex);
    let err = unsafe { CreateIpForwardEntry2(&row) };
    if err != ERROR_SUCCESS {
        return Err(anyhow!("CreateIpForwardEntry2({dest}) failed: {}", err.0));
    }
    Ok(())
}

fn del(dest: Ipv4Addr, gw: Ipv4Addr, ifindex: u32) -> Result<()> {
    let row = forward_row(dest, gw, ifindex);
    let err = unsafe { DeleteIpForwardEntry2(&row) };
    if err != ERROR_SUCCESS && err != ERROR_NOT_FOUND {
        return Err(anyhow!("DeleteIpForwardEntry2({dest}) failed: {}", err.0));
    }
    Ok(())
}

fn set_metric(ifindex: u32, metric: u32) -> Result<()> {
    let mut row = MIB_IPINTERFACE_ROW::default();
    unsafe { InitializeIpInterfaceEntry(&mut row) };
    row.Family = AF_INET;
    row.InterfaceIndex = ifindex;
    let err = unsafe { GetIpInterfaceEntry(&mut row) };
    if err != ERROR_SUCCESS {
        return Err(anyhow!("GetIpInterfaceEntry({ifindex}) failed: {}", err.0));
    }
    // 已知坑：IPv4 Set 前必须 SitePrefixLength=0（MSDN 明示）。
    row.SitePrefixLength = 0;
    row.Metric = metric;
    row.UseAutomaticMetric = false;
    let err = unsafe { SetIpInterfaceEntry(&mut row) };
    if err != ERROR_SUCCESS {
        return Err(anyhow!("SetIpInterfaceEntry({ifindex}) failed: {}", err.0));
    }
    Ok(())
}

/// 保存已加条目与已改接口，成对回滚。
#[derive(Default)]
pub struct RouteGuard {
    added: Vec<Entry>,
    saved_metric: Option<(u32, u32, bool)>, // (ifindex, 原值, 原 UseAutomaticMetric)
    applied_metric_ifindex: Option<u32>,
}

impl RouteGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// 幂等：确保 dests 的 /32 都经 gw@ifindex 存在，多余的删掉；失败仅记日志（warn）。
    pub fn ensure(&mut self, dests: &[Ipv4Addr], gw: Ipv4Addr, ifindex: u32) {
        for d in dests {
            if !self.added.iter().any(|(x, _, _)| x == d) {
                match add(*d, gw, ifindex) {
                    Ok(()) => self.added.push((*d, gw, ifindex)),
                    Err(e) => log::warn!("RouteGuard add {d} failed (kept going): {e:#}"),
                }
            }
        }
        self.added.retain(|(d, g, i)| {
            if dests.contains(d) {
                true
            } else {
                if let Err(e) = del(*d, *g, *i) {
                    log::warn!("RouteGuard del stale {d} failed: {e:#}");
                }
                false
            }
        });
    }

    /// 幂等压制 WLAN 接口 metric 到 target（首次保存原值）；target==0 不动作。
    pub fn set_standby_metric(&mut self, ifindex: u32, target: u32) {
        if target == 0 {
            return;
        }
        if self.applied_metric_ifindex == Some(ifindex) {
            return; // 幂等
        }
        // 先读原值（保存），再压
        let mut row = MIB_IPINTERFACE_ROW::default();
        unsafe { InitializeIpInterfaceEntry(&mut row) };
        row.Family = AF_INET;
        row.InterfaceIndex = ifindex;
        if unsafe { GetIpInterfaceEntry(&mut row) } != ERROR_SUCCESS {
            log::warn!("RouteGuard: read metric of if{ifindex} failed, skip suppression");
            return;
        }
        if let Err(e) = set_metric(ifindex, target) {
            log::warn!("RouteGuard suppress metric if{ifindex} -> {target} failed: {e:#}");
            return;
        }
        self.saved_metric = Some((ifindex, row.Metric, row.UseAutomaticMetric));
        self.applied_metric_ifindex = Some(ifindex);
    }

    fn restore_metric(&mut self) {
        if let Some(&(ifindex, metric, auto)) = self.saved_metric.as_ref() {
            if let Err(e) = set_metric(ifindex, metric) {
                // 还原失败：保留 saved_metric（下次还原/teardown 重试）并清
                // applied 标志，让下一个 standby 拍重新压制而非永久短路
                // （回滚铁律：失败不吞状态）。
                log::warn!(
                    "RouteGuard restore metric if{ifindex} -> {metric} failed (kept for retry): {e:#}"
                );
                self.applied_metric_ifindex = None;
                return;
            }
            self.saved_metric = None;
            log::info!("RouteGuard restored metric if{ifindex} -> {metric}");
            // UseAutomaticMetric 还原（auto 时交还系统）
            if auto {
                let mut row = MIB_IPINTERFACE_ROW::default();
                unsafe { InitializeIpInterfaceEntry(&mut row) };
                row.Family = AF_INET;
                row.InterfaceIndex = ifindex;
                if unsafe { GetIpInterfaceEntry(&mut row) } == ERROR_SUCCESS {
                    row.SitePrefixLength = 0;
                    row.UseAutomaticMetric = true;
                    unsafe {
                        let _ = SetIpInterfaceEntry(&mut row);
                    }
                }
            }
        }
        self.applied_metric_ifindex = None;
    }

    /// 只还原 metric，不动路由：exclusive 模式让位瞬间（有线恢复、WLAN 仍短暂在线）
    /// 路由是否保留由后续 ensure/teardown 决定，此处仅解除接口压制。
    pub fn release_metric(&mut self) {
        self.restore_metric();
    }

    /// 全量回滚：删已加路由 + 还原 metric。teardown 后 Guard 可复用。
    pub fn teardown(&mut self) {
        for (d, g, i) in self.added.drain(..) {
            if let Err(e) = del(d, g, i) {
                log::warn!("RouteGuard teardown {d} failed: {e:#}");
            }
        }
        self.restore_metric();
    }
}

/// 启动清残留：删指向 dests 的 /32（仅 NETMGMT 协议，避免误删用户静态路由）。
pub fn cleanup_stale(dests: &[Ipv4Addr]) {
    let mut table: *mut MIB_IPFORWARD_TABLE2 = std::ptr::null_mut();
    if unsafe { GetIpForwardTable2(AF_INET, &mut table) } != ERROR_SUCCESS {
        return;
    }
    let n = unsafe { (*table).NumEntries } as usize;
    // 定长字段 [MIB_IPFORWARD_ROW2; 1] 不能直接索引（>1 条路由即越界 panic，
    // 与 wlan.rs InterfaceInfo 同一教训——2026-09-10 真机：本机 ~100 条路由，
    // manager 启动即崩且 tokio 静默吞掉）。用变长视图读整个表。
    let rows = unsafe { std::slice::from_raw_parts((*table).Table.as_ptr(), n) };
    for row in rows {
        if row.Protocol != MIB_IPPROTO_NETMGMT || row.DestinationPrefix.PrefixLength != 32 {
            continue;
        }
        let hop_ip =
            crate::wireless::s_addr_to_ipv4(unsafe { row.NextHop.Ipv4.sin_addr.S_un.S_addr });
        let dest_ip = crate::wireless::s_addr_to_ipv4(unsafe {
            row.DestinationPrefix.Prefix.Ipv4.sin_addr.S_un.S_addr
        });
        if dests.contains(&dest_ip) {
            if let Err(e) = del(dest_ip, hop_ip, row.InterfaceIndex) {
                log::warn!("cleanup_stale del {dest_ip} failed: {e:#}");
            } else {
                log::info!("Removed stale /32 route to {dest_ip} via {hop_ip}");
            }
        }
    }
    unsafe { windows::Win32::NetworkManagement::IpHelper::FreeMibTable(table.cast()) };
}
