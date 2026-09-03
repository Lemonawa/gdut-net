//! Dr.COM（GDUT 变体）心跳报文规格。
//!
//! 协议来源：ADR-0002（gdut-drcom auth.c 与 drcom-generic issue #82 真实抓包交叉验证）。
//! 纯字节操作，无 IO。实现从提炼的协议常量表与偏移出发，不移植任何 GPL/AGPL 代码。

use md5::{Digest, Md5};

/// 心跳服务器 UDP 端口。
pub const PORT: u16 = 61440;
/// KA1 pkt2 报文长度。
pub const KA1_LEN: usize = 96;
/// KA2 报文长度。
pub const KA2_LEN: usize = 40;

pub type Seed = [u8; 4];
pub type HostIp = [u8; 4];
pub type Key = [u8; 4];
pub type Flag = [u8; 2];

/// 心跳计数器推进：`(cnt + 2) % 128`。
///
/// 注：任务简报正文写 `(cnt+2)%127`，与其自带测试向量
/// `next_cnt(125)==127`、`next_cnt(127)==1` 矛盾（唯一吻合的是 `%128`）；以测试为准。
pub fn next_cnt(cnt: u8) -> u8 {
    (cnt + 2) % 128
}

/// KA1 探测报文：`07 cnt 08 00 01 00 00 00`。
pub fn ka1_pkt1(cnt: u8) -> [u8; 8] {
    [0x07, cnt, 0x08, 0x00, 0x01, 0x00, 0x00, 0x00]
}

/// KA1 探测响应解析：`pkt[0]==0x07 && len>=30`；seed=pkt[8..12]，
/// host_ip=pkt[12..16]；`pkt[2]==0x10` 时为文件报文，flag=Some(pkt[6..8])。
pub fn parse_ka1_resp(pkt: &[u8]) -> Option<Ka1Init> {
    if pkt.first() != Some(&0x07) || pkt.len() < 30 {
        return None;
    }
    let mut seed = [0u8; 4];
    seed.copy_from_slice(&pkt[8..12]);
    let mut host_ip = [0u8; 4];
    host_ip.copy_from_slice(&pkt[12..16]);
    let flag = (pkt[2] == 0x10).then(|| [pkt[6], pkt[7]]);
    Some(Ka1Init {
        seed,
        host_ip,
        flag,
    })
}

/// KA1 探测响应携带的初始化信息。
pub struct Ka1Init {
    pub seed: Seed,
    pub host_ip: HostIp,
    pub flag: Option<Flag>,
}

/// 校验字节：`seed[0]&3` 选择模式——
/// 0: 明文 `le32(20000711)+le32(126)`；1: MD5 挑 `[2,3,8,9,5,6,13,14]`；
/// 2: MD4 挑 `[1,2,8,9,4,5,11,12]`；3: SHA1 挑 `[2,3,9,10,5,6,15,16]`。
pub fn crypt_bytes(seed: &Seed) -> [u8; 8] {
    match seed[0] & 3 {
        0 => plain_bytes(),
        1 => md5_bytes(seed),
        2 => md4_bytes(seed),
        _ => sha1_bytes(seed),
    }
}

/// 模式 0 明文校验字节。
pub fn plain_bytes() -> [u8; 8] {
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&20000711u32.to_le_bytes());
    out[4..].copy_from_slice(&126u32.to_le_bytes());
    out
}

/// 模式 1：MD5(seed) 按下标挑 8 字节。
pub fn md5_bytes(seed: &Seed) -> [u8; 8] {
    let d = Md5::digest(seed);
    [d[2], d[3], d[8], d[9], d[5], d[6], d[13], d[14]]
}

/// 模式 2：MD4(seed) 按下标挑 8 字节。
pub fn md4_bytes(seed: &Seed) -> [u8; 8] {
    let d = md4::Md4::digest(seed);
    [d[1], d[2], d[8], d[9], d[4], d[5], d[11], d[12]]
}

/// 模式 3：SHA1(seed) 按下标挑 8 字节。
///
/// 注：简报正文写的是对 KA1 pkt2 报文前缀做 SHA1，但独立脚本对
/// issue #82 权威抓包反推，唯一命中区间为 SHA1(seed@20..24)——
/// 即仅对 4 字节 seed 摘要，与 crypt 模式 1/2 的输入一致。
/// 抓包校验值 `9ae9cef84b020aa3` 据此精确复现。
pub fn sha1_bytes(seed: &Seed) -> [u8; 8] {
    let d = sha1::Sha1::digest(seed);
    [d[2], d[3], d[9], d[10], d[5], d[6], d[15], d[16]]
}

/// KA1 pkt2（96B）：`07 cnt 60 00 03 00`+uid 零 10B+`host_ip@12`+
/// `00 62|63 flag@16..20`+`seed@20..24`+`checksum@24..32`+零 64B。
///
/// `first`（首次心跳）选 `0x62`，否则 `0x63`。
pub fn ka1_pkt2(cnt: u8, first: bool, host_ip: HostIp, seed: Seed, flag: Flag) -> [u8; KA1_LEN] {
    let mut pkt = [0u8; KA1_LEN];
    pkt[0] = 0x07;
    pkt[1] = cnt;
    pkt[2] = 0x60;
    pkt[3] = 0x00;
    pkt[4] = 0x03;
    pkt[5] = 0x00;
    pkt[12..16].copy_from_slice(&host_ip);
    pkt[16] = 0x00;
    pkt[17] = if first { 0x62 } else { 0x63 };
    pkt[18..20].copy_from_slice(&flag);
    pkt[20..24].copy_from_slice(&seed);
    pkt[24..32].copy_from_slice(&crypt_bytes(&seed));
    pkt
}

/// KA2 校验和：报文按 16 位小端字全包 XOR → `&0xffff` → `*0x2c7` → 32 位小端写出。
pub fn ka2_checksum(pkt: &[u8; KA2_LEN]) -> [u8; 4] {
    let mut sum: u16 = 0;
    for chunk in pkt.as_chunks::<2>().0 {
        sum ^= u16::from_le_bytes([chunk[0], chunk[1]]);
    }
    let v = u32::from(sum).wrapping_mul(0x2c7);
    v.to_le_bytes()
}

/// KA2 type1 报文（40B）：`07 cnt 28 00 0b 01 flag rand@8..10 零6B key@16..20 零`。
pub fn ka2_pkt1(cnt: u8, flag: Flag, rand: u16, key: Key) -> [u8; KA2_LEN] {
    let mut pkt = [0u8; KA2_LEN];
    pkt[0] = 0x07;
    pkt[1] = cnt;
    pkt[2] = 0x28;
    pkt[3] = 0x00;
    pkt[4] = 0x0b;
    pkt[5] = 0x01;
    pkt[6..8].copy_from_slice(&flag);
    pkt[8..10].copy_from_slice(&rand.to_be_bytes());
    pkt[16..20].copy_from_slice(&key);
    pkt
}

/// KA2 响应解析：`pkt[0]==0x07 && len>=20`；key=pkt[16..20]；
/// `pkt[2]==0x10` 即文件报文，flag=Some(pkt[6..8])。
pub fn parse_ka2_resp(pkt: &[u8]) -> Option<Ka2Resp> {
    if pkt.first() != Some(&0x07) || pkt.len() < 20 {
        return None;
    }
    let mut key = [0u8; 4];
    key.copy_from_slice(&pkt[16..20]);
    let flag = (pkt[2] == 0x10).then(|| [pkt[6], pkt[7]]);
    Some(Ka2Resp { key, flag })
}

/// KA2 响应携带的密钥与可选文件报文 flag。
pub struct Ka2Resp {
    pub key: Key,
    pub flag: Option<Flag>,
}

/// KA2 type3 报文（40B）：`07 cnt 28 00 0b 03 flag rand 零6B key 零4B crc@24..28
/// host_ip@28..32 零8B`；crc 对校验位全零的全包计算。
pub fn ka2_pkt2(cnt: u8, flag: Flag, rand: u16, key: Key, host_ip: HostIp) -> [u8; KA2_LEN] {
    let mut pkt = [0u8; KA2_LEN];
    pkt[0] = 0x07;
    pkt[1] = cnt;
    pkt[2] = 0x28;
    pkt[3] = 0x00;
    pkt[4] = 0x0b;
    pkt[5] = 0x03;
    pkt[6..8].copy_from_slice(&flag);
    pkt[8..10].copy_from_slice(&rand.to_be_bytes());
    pkt[16..20].copy_from_slice(&key);
    pkt[28..32].copy_from_slice(&host_ip);
    let crc = ka2_checksum(&pkt);
    pkt[24..28].copy_from_slice(&crc);
    pkt
}
