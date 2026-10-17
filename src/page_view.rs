use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc};

use gpui_kit::*;
use reflexo::hash::Fingerprint;

use crate::preview::VectorPage;

const TILE: u32 = 512;
const MAX_TILES: usize = 96; // at most 96 MiB of CPU pixel buffers

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    page: Fingerprint,
    scale: u32,
    size: (u32, u32),
    x: u32,
    y: u32,
}

struct Tile {
    image: Arc<RenderImage>,
    used: u64,
}

#[derive(Default)]
pub struct TileCache {
    tiles: HashMap<Key, Tile>,
    clock: u64,
}

impl TileCache {
    fn tile(&mut self, page: &VectorPage, scale: f32, x: u32, y: u32) -> Arc<RenderImage> {
        self.clock += 1;
        let key = Key {
            page: page.id,
            scale: scale.to_bits(),
            size: (page.width().to_bits(), page.height().to_bits()),
            x,
            y,
        };
        if let Some(tile) = self.tiles.get_mut(&key) {
            tile.used = self.clock;
            return tile.image.clone();
        }
        let image = raster_tile(page, scale, x, y);
        // New vector content often changes only a few tiles. Equal tiles keep
        // their GPU texture identity across document updates.
        let image = self
            .tiles
            .iter()
            .find_map(|(old, tile)| {
                (old.scale == key.scale
                    && old.x == x
                    && old.y == y
                    && tile.image.as_bytes(0) == image.as_bytes(0))
                .then(|| tile.image.clone())
            })
            .unwrap_or_else(|| Arc::new(image));
        self.tiles.insert(
            key,
            Tile {
                image: image.clone(),
                used: self.clock,
            },
        );
        image
    }

    fn trim(&mut self, window: &mut Window, cx: &mut App) {
        while self.tiles.len() > MAX_TILES {
            let key = *self
                .tiles
                .iter()
                .min_by_key(|(_, tile)| tile.used)
                .unwrap()
                .0;
            let removed = self.tiles.remove(&key).unwrap();
            if !self
                .tiles
                .values()
                .any(|tile| tile.image.id == removed.image.id)
            {
                cx.drop_image(removed.image, Some(window));
            }
        }
    }
}

/// Rasterize only this tile, directly from the retained SVG glyph paths. The
/// transform includes zoom and display DPR; no full-page bitmap is enlarged.
fn raster_tile(page: &VectorPage, scale: f32, x: u32, y: u32) -> RenderImage {
    let mut pixmap = resvg::tiny_skia::Pixmap::new(TILE, TILE).unwrap();
    pixmap.fill(resvg::tiny_skia::Color::WHITE);
    let transform = resvg::tiny_skia::Transform::from_row(
        scale,
        0.,
        0.,
        scale,
        -((x * TILE) as f32),
        -((y * TILE) as f32),
    );
    resvg::render(&page.tree, transform, &mut pixmap.as_mut());
    let mut pixels = pixmap.take();
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    } // GPUI uses BGRA
    RenderImage::new(vec![image::Frame::new(
        image::RgbaImage::from_raw(TILE, TILE, pixels).unwrap(),
    )])
}

pub fn page_canvas(
    page: Arc<VectorPage>,
    width: f32,
    cache: Rc<RefCell<TileCache>>,
) -> impl IntoElement {
    let height = width * page.height() / page.width();
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            let visible = bounds.intersect(&window.content_mask().bounds);
            if visible.size.width <= px(0.) || visible.size.height <= px(0.) {
                return;
            }
            let dpr = window.scale_factor();
            let scale = width / page.width() * dpr;
            let x0 = (f32::from(visible.left() - bounds.left()) * dpr / TILE as f32)
                .floor()
                .max(0.) as u32;
            let y0 = (f32::from(visible.top() - bounds.top()) * dpr / TILE as f32)
                .floor()
                .max(0.) as u32;
            let x1 = (f32::from(visible.right() - bounds.left()) * dpr / TILE as f32).ceil() as u32;
            let y1 = (f32::from(visible.bottom() - bounds.top()) * dpr / TILE as f32).ceil() as u32;
            let tile_size = TILE as f32 / dpr;
            let mut cache = cache.borrow_mut();
            for y in y0..y1 {
                for x in x0..x1 {
                    let image = cache.tile(&page, scale, x, y);
                    let tile = Bounds::new(
                        bounds.origin + point(px(x as f32 * tile_size), px(y as f32 * tile_size)),
                        size(px(tile_size), px(tile_size)),
                    );
                    let _ = window.paint_image(
                        bounds.intersect(&tile),
                        tile,
                        Corners::default(),
                        image,
                        0,
                        false,
                    );
                }
            }
            cache.trim(window, cx);
        },
    )
    .w(px(width))
    .h(px(height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn zoom_rasterizes_vectors_and_identical_tiles_reuse_textures() {
        let tree = resvg::usvg::Tree::from_str(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><path d="M10 0V100" stroke="black" stroke-width="0.2"/></svg>"#,
            &resvg::usvg::Options::default(),
        ).unwrap();
        let mut page = VectorPage {
            id: Fingerprint::from_pair(1, 0),
            tree,
        };
        let mut cache = TileCache::default();
        let normal = cache.tile(&page, 1., 0, 0);
        let zoomed = cache.tile(&page, 10., 0, 0);
        assert_ne!(normal.as_bytes(0), zoomed.as_bytes(0));
        let bytes = zoomed.as_bytes(0).unwrap();
        assert!(bytes[(10 * TILE as usize + 100) * 4] < 100);
        assert_eq!(bytes[(10 * TILE as usize + 10) * 4], 255);
        page.id = Fingerprint::from_pair(2, 0);
        assert_eq!(cache.tile(&page, 10., 0, 0).id, zoomed.id);
    }
}
