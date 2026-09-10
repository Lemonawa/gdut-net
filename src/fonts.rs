//! egui 中文字体：从系统字体目录加载一个 CJK 字体并注入字体表。
//! epaint 0.36 用 skrifa 解析，支持 .ttc（index 0）；候选顺序先单文件 TTF。

use anyhow::{bail, Context, Result};

// `egui` 不是直接依赖；tray 面板同款用法，走 eframe 的再导出。
use eframe::egui;

const CANDIDATES: [&str; 4] = ["Deng.ttf", "simhei.ttf", "msyh.ttc", "simsun.ttc"];

pub fn install_cjk_fonts(ctx: &egui::Context) -> Result<String> {
    let fonts_dir =
        std::path::PathBuf::from(std::env::var_os("SystemRoot").context("SystemRoot is not set")?)
            .join("Fonts");
    for name in CANDIDATES {
        let path = fonts_dir.join(name);
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let mut defs = egui::FontDefinitions::default();
        defs.font_data.insert(
            "cjk".to_string(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            defs.families
                .entry(family)
                .or_default()
                .push("cjk".to_string());
        }
        ctx.set_fonts(defs);
        log::info!("Loaded CJK font {name} from {}", fonts_dir.display());
        return Ok(name.to_string());
    }
    bail!(
        "No CJK font found in {} (tried {CANDIDATES:?})",
        fonts_dir.display()
    )
}
