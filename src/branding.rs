use gpui_kit::{AssetSource, SharedString};
use std::{
    borrow::Cow,
    sync::{Arc, OnceLock},
};

pub const NAME: &str = "Folio";
pub const APP_ID: &str = if cfg!(target_os = "macos") {
    "de.silas.folio"
} else {
    "folio"
};
pub const ICON_PATH: &str = "branding/folio-invoices-coral.svg";
const ICON: &[u8] = include_bytes!("../assets/folio-invoices-coral.svg");

fn raster_icon(size: u32) -> anyhow::Result<resvg::tiny_skia::Pixmap> {
    let tree = resvg::usvg::Tree::from_data(ICON, &resvg::usvg::Options::default())?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size)
        .ok_or_else(|| anyhow::anyhow!("Invalid icon dimensions"))?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(
            size as f32 / tree.size().width(),
            size as f32 / tree.size().height(),
        ),
        &mut pixmap.as_mut(),
    );
    Ok(pixmap)
}

/// Used by the bundle script before generating the platform icon sizes.
pub fn write_icon(path: &std::path::Path) -> anyhow::Result<()> {
    raster_icon(1024)?.save_png(path)?;
    Ok(())
}

/// Ship the color app icon alongside the complete GPUI Kit component icons.
pub struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if path == ICON_PATH {
            return Ok(Some(Cow::Borrowed(ICON)));
        }
        gpui_kit::assets::AllAssets.load(path)
    }
    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut assets = gpui_kit::assets::AllAssets.list(path)?;
        if ICON_PATH.starts_with(path) {
            assets.push(ICON_PATH.into());
        }
        Ok(assets)
    }
}

/// X11 takes a raster icon; the checked-in source remains entirely vector.
/// Wayland resolves APP_ID through folio.desktop and the scalable icon theme.
pub fn window_icon() -> Arc<image::RgbaImage> {
    static ICON_IMAGE: OnceLock<Arc<image::RgbaImage>> = OnceLock::new();
    ICON_IMAGE
        .get_or_init(|| {
            let pixmap = raster_icon(256).expect("Folio icon is valid SVG");
            let mut bytes = pixmap.take();
            // tiny-skia renders premultiplied RGBA, X11's _NET_WM_ICON expects straight alpha.
            for pixel in bytes.as_chunks_mut::<4>().0 {
                let alpha = pixel[3] as u32;
                for channel in &mut pixel[..3] {
                    *channel = (u32::from(*channel) * 255 + alpha / 2)
                        .checked_div(alpha)
                        .unwrap_or(0)
                        .min(255) as u8;
                }
            }
            Arc::new(image::RgbaImage::from_raw(256, 256, bytes).unwrap())
        })
        .clone()
}
