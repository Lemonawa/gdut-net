//! gdut-net-pack — 把主程序与脚本追加到 setup.exe 尾部，产出单文件发布物。
//! Host 工具：不依赖 Windows，CI 与本地 xwin 构建后均可运行。
//! Usage: gdut-net-pack --setup <setup.exe> --payload-dir <dir> [--file <f>]... --out <dist.exe>

use anyhow::Context as _;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut setup: Option<std::path::PathBuf> = None;
    let mut payload_dir: Option<std::path::PathBuf> = None;
    let mut extras: Vec<std::path::PathBuf> = Vec::new();
    let mut out: Option<std::path::PathBuf> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--setup" => setup = args.next().map(Into::into),
            "--payload-dir" => payload_dir = args.next().map(Into::into),
            "--file" => extras.push(args.next().context("--file needs a value")?.into()),
            "--out" => out = args.next().map(Into::into),
            other => anyhow::bail!("unknown argument {other:?}"),
        }
    }
    let setup = setup.context("--setup is required")?;
    let payload_dir = payload_dir.context("--payload-dir is required")?;
    let out = out.context("--out is required")?;
    gdut_net::packaging::pack_into_file(&setup, &extras, &payload_dir, &out)?;
    println!(
        "Packed {} (+{} extras) -> {}",
        setup.display(),
        extras.len(),
        out.display()
    );
    Ok(())
}
