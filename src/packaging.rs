//! 打包文件级逻辑（纯 std）：收集 payload 目录 + 额外文件，写单文件发布物。

use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::payload::{pack, validate_name, Entry};

/// 读目录下所有常规文件（不递归），按名称排序保证产物可复现。
pub fn collect_dir(dir: &Path) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in
        std::fs::read_dir(dir).with_context(|| format!("Failed to read {}", dir.display()))?
    {
        let item = item?;
        let path = item.path();
        if !item.file_type()?.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .context("Non-UTF-8 file name")?
            .to_string();
        validate_name(&name)?;
        let data =
            std::fs::read(&path).with_context(|| format!("Failed to read {}", path.display()))?;
        entries.push(Entry { name, data });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// setup 原文件 + extras（按 basename 收编）+ payload 目录 → out。
pub fn pack_into_file(
    setup: &Path,
    extras: &[std::path::PathBuf],
    payload_dir: &Path,
    out: &Path,
) -> Result<()> {
    let setup_bytes =
        std::fs::read(setup).with_context(|| format!("Failed to read {}", setup.display()))?;
    let mut entries = collect_dir(payload_dir)?;
    for extra in extras {
        let name = extra
            .file_name()
            .and_then(|n| n.to_str())
            .context("Non-UTF-8 extra file name")?
            .to_string();
        validate_name(&name)?;
        if entries.iter().any(|e| e.name == name) {
            bail!("duplicate payload entry {name:?}");
        }
        let data =
            std::fs::read(extra).with_context(|| format!("Failed to read {}", extra.display()))?;
        entries.push(Entry { name, data });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    let bytes = pack(
        &setup_bytes,
        &entries
            .iter()
            .map(|e| (e.name.clone(), e.data.clone()))
            .collect::<Vec<_>>(),
    )?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(out, bytes).with_context(|| format!("Failed to write {}", out.display()))?;
    Ok(())
}
