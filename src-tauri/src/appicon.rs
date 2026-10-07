//! Caches source-app icons (64px PNG) and a representative color per app.

use crate::platform;
use image::GenericImageView;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct AppLook {
    pub icon: Option<String>,
    pub color: Option<String>,
}

pub struct AppIconCache {
    dir: PathBuf,
    looks: HashMap<String, AppLook>,
}

impl AppIconCache {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, looks: HashMap::new() }
    }

    pub fn look(&mut self, bundle_id: &str, bundle_path: &str) -> AppLook {
        if let Some(l) = self.looks.get(bundle_id) {
            return l.clone();
        }
        let look = self.build(bundle_id, bundle_path).unwrap_or_else(|| {
            eprintln!("[pastel] no icon for {bundle_id} ({bundle_path})");
            AppLook { icon: None, color: None }
        });
        self.looks.insert(bundle_id.to_string(), look.clone());
        look
    }

    fn build(&self, bundle_id: &str, bundle_path: &str) -> Option<AppLook> {
        let png_path = self.dir.join(format!("{bundle_id}.png"));
        let img = if png_path.exists() {
            image::open(&png_path).ok()?
        } else {
            let bytes = platform::app_icon_bytes(bundle_path)?;
            let img = image::load_from_memory(&bytes)
                .map_err(|e| eprintln!("[pastel] icon decode failed for {bundle_id}: {e}"))
                .ok()?;
            let img = img.resize_exact(64, 64, image::imageops::FilterType::Lanczos3);
            img.save(&png_path).ok()?;
            img
        };
        Some(AppLook {
            icon: Some(png_path.to_string_lossy().into_owned()),
            color: Some(dominant_color(&img)),
        })
    }
}

/// Average of the saturated opaque pixels; falls back to all opaque pixels
/// for monochrome icons. Darkened a little when too bright for white text.
fn dominant_color(img: &image::DynamicImage) -> String {
    let mut sat = (0u64, 0u64, 0u64, 0u64);
    let mut all = (0u64, 0u64, 0u64, 0u64);
    for (_, _, p) in img.pixels() {
        let [r, g, b, a] = p.0;
        if a < 200 {
            continue;
        }
        let max = r.max(g).max(b) as f32;
        let min = r.min(g).min(b) as f32;
        let s = if max == 0.0 { 0.0 } else { (max - min) / max };
        all = (all.0 + r as u64, all.1 + g as u64, all.2 + b as u64, all.3 + 1);
        if s > 0.3 && max > 60.0 {
            sat = (sat.0 + r as u64, sat.1 + g as u64, sat.2 + b as u64, sat.3 + 1);
        }
    }
    let (r, g, b, n) = if sat.3 * 8 >= all.3.max(1) { sat } else { all };
    if n == 0 {
        return "#8e8e93".into();
    }
    let (mut r, mut g, mut b) = ((r / n) as f32, (g / n) as f32, (b / n) as f32);
    let lum = (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0;
    if lum > 0.8 {
        let k = 0.8 / lum;
        r *= k;
        g *= k;
        b *= k;
    }
    format!("#{:02x}{:02x}{:02x}", r as u8, g as u8, b as u8)
}

pub fn ensure_dir(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
}
