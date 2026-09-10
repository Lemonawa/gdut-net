//! Windows SOCKADDR_IN 字节序换算的回归测试（Linux 可跑，纯函数）。
//!
//! 2026-09-10 真机事故：`S_addr: u32::from(ip)` 直接存让 LE 内存变成字节反序，
//! 路由表里 10.0.3.2 被写成 2.3.0.10（`route print` 实锤），portal 永远 NO REPLY。

use std::net::Ipv4Addr;

use gdut_net::wireless::{ipv4_to_s_addr, s_addr_to_ipv4};

#[test]
fn s_addr_memory_bytes_are_network_order() {
    let ip = Ipv4Addr::new(10, 0, 3, 2);
    let raw = ipv4_to_s_addr(ip);
    // 内存字节必须是点分顺序（10,0,3,2）——Win32 读的就是这 4 个字节。
    assert_eq!(raw.to_le_bytes(), [10, 0, 3, 2]);
    // 旧 bug 的值（LE 内存 [2,3,0,10]）：保留断言防止回归到这条路径。
    assert_ne!(u32::from(ip).to_le_bytes(), [10, 0, 3, 2]);
}

#[test]
fn s_addr_roundtrip_and_table_readback() {
    for ip in [
        Ipv4Addr::new(10, 0, 3, 2),
        Ipv4Addr::new(223, 5, 5, 5),
        Ipv4Addr::new(10, 43, 0, 1),
        Ipv4Addr::UNSPECIFIED,
        Ipv4Addr::new(255, 255, 255, 255),
    ] {
        assert_eq!(s_addr_to_ipv4(ipv4_to_s_addr(ip)), ip);
    }
    // GetIpForwardTable2 读回的值：内存 [10,0,3,2] 在 LE 上按 u32 读出 = 0x0203000A。
    assert_eq!(s_addr_to_ipv4(0x0203_000A), Ipv4Addr::new(10, 0, 3, 2));
}
