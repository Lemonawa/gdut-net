//! 单文件发布载荷容器（纯逻辑）：[setup][data...][TOC][footer]。
//! footer 24B: "GDUTPAK1" | u32 version | u32 count | u64 toc_offset。
//! TOC 条目: u16 name_len | name(UTF-8) | u64 offset | u64 len | sha256[32]。

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

pub const MAGIC: &[u8; 8] = b"GDUTPAK1";
const VERSION: u32 = 1;
pub const FOOTER_LEN: usize = 24;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub data: Vec<u8>,
}

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().into()
}

/// 文件名白名单：非空、无路径分隔符、无 `..`、无 ASCII 控制字符。
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("payload entry name is empty");
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") || name.contains(':') {
        bail!("payload entry name {name:?} must not contain path separators or \"..\"");
    }
    if name.chars().any(|c| c.is_control()) {
        bail!("payload entry name {name:?} contains control characters");
    }
    Ok(())
}

pub fn pack(setup: &[u8], entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut seen = std::collections::HashSet::new();
    for (name, _) in entries {
        validate_name(name)?;
        if !seen.insert(name.clone()) {
            bail!("duplicate payload entry name {name:?}");
        }
    }
    let mut out = setup.to_vec();
    let mut toc: Vec<u8> = Vec::new();
    for (name, data) in entries {
        let offset = out.len() as u64;
        out.extend_from_slice(data);
        let name_bytes = name.as_bytes();
        let name_len = u16::try_from(name_bytes.len()).context("payload name too long")?;
        toc.extend_from_slice(&name_len.to_le_bytes());
        toc.extend_from_slice(name_bytes);
        toc.extend_from_slice(&offset.to_le_bytes());
        toc.extend_from_slice(&(data.len() as u64).to_le_bytes());
        toc.extend_from_slice(&sha256(data));
    }
    let toc_offset = out.len() as u64;
    out.extend_from_slice(&toc);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    out.extend_from_slice(&toc_offset.to_le_bytes());
    Ok(out)
}

/// 读 payload；无 footer / magic 不符 → Ok(None)（开发态未打包）。
/// magic 相符但结构损坏 → Err（拒绝半安装）。
pub fn unpack(exe: &[u8]) -> Result<Option<Vec<Entry>>> {
    if exe.len() < FOOTER_LEN {
        return Ok(None);
    }
    let footer = &exe[exe.len() - FOOTER_LEN..];
    if &footer[..8] != MAGIC {
        return Ok(None);
    }
    let version = u32::from_le_bytes(footer[8..12].try_into().unwrap());
    if version != VERSION {
        bail!("unsupported payload version {version}");
    }
    let count = u32::from_le_bytes(footer[12..16].try_into().unwrap()) as usize;
    let toc_offset = u64::from_le_bytes(footer[16..24].try_into().unwrap()) as usize;
    let toc_end = exe.len() - FOOTER_LEN;
    if toc_offset > toc_end {
        bail!("payload TOC offset {toc_offset} out of range (len {toc_end})");
    }
    let toc = &exe[toc_offset..toc_end];
    let mut pos = 0usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        if pos + 2 > toc.len() {
            bail!("payload TOC truncated");
        }
        let name_len = u16::from_le_bytes(toc[pos..pos + 2].try_into().unwrap()) as usize;
        pos += 2;
        if pos + name_len + 8 + 8 + 32 > toc.len() {
            bail!("payload TOC entry truncated");
        }
        let name = std::str::from_utf8(&toc[pos..pos + name_len])
            .context("payload entry name is not UTF-8")?
            .to_string();
        validate_name(&name)?;
        pos += name_len;
        let offset = u64::from_le_bytes(toc[pos..pos + 8].try_into().unwrap()) as usize;
        pos += 8;
        let len = u64::from_le_bytes(toc[pos..pos + 8].try_into().unwrap()) as usize;
        pos += 8;
        let want: [u8; 32] = toc[pos..pos + 32].try_into().unwrap();
        pos += 32;
        if offset.checked_add(len).is_none_or(|end| end > toc_offset) {
            bail!("payload entry {name:?} out of data range");
        }
        let data = &exe[offset..offset + len];
        if sha256(data) != want {
            bail!("payload checksum mismatch for {name:?}");
        }
        entries.push(Entry {
            name,
            data: data.to_vec(),
        });
    }
    if pos != toc.len() {
        bail!("payload TOC has trailing bytes");
    }
    Ok(Some(entries))
}
