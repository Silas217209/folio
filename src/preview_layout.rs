use gpui_kit::{Bounds, Pixels, Point, Size, point, px, size};

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub enum FitMode {
    #[default]
    Manual,
    Width,
    Page,
}

impl FitMode {
    pub fn zoom(self, dimensions: &[(f32, f32)], viewport: Size<Pixels>, manual: f32) -> f32 {
        let Some(first) = dimensions.first() else {
            return manual;
        };
        if viewport.width <= px(48.) || viewport.height <= px(48.) {
            return manual;
        }
        let width = dimensions.iter().map(|p| p.0).fold(0., f32::max);
        let height = dimensions.iter().map(|p| p.1).fold(0., f32::max);
        if width <= 0. || height <= 0. || first.0 <= 0. {
            return manual;
        }
        let width_scale = (f32::from(viewport.width) - 48.) / width;
        let scale = match self {
            Self::Manual => return manual,
            Self::Width => width_scale,
            Self::Page => width_scale.min((f32::from(viewport.height) - 48.) / height),
        };
        scale * first.0 / 500.
    }
}

/// Shared geometry for continuous pages, source following, and anchored zoom.
/// Page gaps and outer padding stay fixed as the document scale changes.
pub struct PreviewLayout {
    pub pages: Vec<Bounds<Pixels>>,
    pub size: Size<Pixels>,
    pub scale: f32,
}

impl PreviewLayout {
    pub fn new(dimensions: &[(f32, f32)], zoom: f32, viewport: Size<Pixels>) -> Self {
        let scale = 500. * zoom / dimensions.first().map_or(500., |page| page.0);
        let width = dimensions
            .iter()
            .map(|page| page.0 * scale)
            .fold(0., f32::max);
        let width = px(width + 48.).max(viewport.width);
        let mut top = px(24.);
        let pages = dimensions
            .iter()
            .map(|&(w, h)| {
                let page = Bounds::new(
                    point((width - px(w * scale)) / 2., top),
                    size(px(w * scale), px(h * scale)),
                );
                top += page.size.height + px(24.);
                page
            })
            .collect();
        Self {
            pages,
            size: size(width, top.max(viewport.height)),
            scale,
        }
    }

    pub fn clamp_offset(&self, offset: Point<Pixels>, viewport: Size<Pixels>) -> Point<Pixels> {
        point(
            offset
                .x
                .clamp(-(self.size.width - viewport.width).max(px(0.)), px(0.)),
            offset
                .y
                .clamp(-(self.size.height - viewport.height).max(px(0.)), px(0.)),
        )
    }

    pub fn zoom_offset(
        &self,
        next: &Self,
        offset: Point<Pixels>,
        anchor: Point<Pixels>,
        viewport: Size<Pixels>,
    ) -> Point<Pixels> {
        let position = anchor - offset;
        let index = self
            .pages
            .iter()
            .position(|page| page.bottom() >= position.y)
            .or_else(|| self.pages.len().checked_sub(1));
        let Some(index) = index else { return offset };
        let old = self.pages[index];
        let new = next.pages[index];
        let ratio = next.scale / self.scale;
        next.clamp_offset(
            anchor - (new.origin + (position - old.origin) * ratio),
            viewport,
        )
    }

    pub fn follow_offset(
        &self,
        page: usize,
        x: f32,
        y: f32,
        offset: Point<Pixels>,
        viewport: Size<Pixels>,
    ) -> Option<Point<Pixels>> {
        let page = self.pages.get(page)?;
        let position = page.origin + point(px(x * self.scale), px(y * self.scale));
        let visible = position + offset;
        let inset = px(72.).min(viewport.height / 4.).min(viewport.width / 4.);
        let mut target = offset;
        if visible.y < inset || visible.y > viewport.height - inset {
            target.y = viewport.height * 0.35 - position.y;
        }
        if visible.x < inset || visible.x > viewport.width - inset {
            target.x = viewport.width / 2. - position.x;
        }
        let target = self.clamp_offset(target, viewport);
        (target != offset).then_some(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn continuous_pages_follow_and_zoom_with_fixed_gaps_and_mixed_sizes() {
        let viewport = size(px(600.), px(700.));
        let dimensions = [(500., 700.), (400., 900.), (600., 300.)];
        let layout = PreviewLayout::new(&dimensions, 1., viewport);
        assert_eq!(layout.pages[1].top(), px(748.));
        assert_eq!(layout.pages[1].left(), px(124.));
        assert_eq!(layout.size.height, px(1996.));
        let offset = layout
            .follow_offset(1, 100., 100., Point::default(), viewport)
            .unwrap();
        assert_eq!(offset.y, px(-603.));
        assert!(
            layout
                .follow_offset(1, 100., 100., offset, viewport)
                .is_none()
        );
        let zoomed = PreviewLayout::new(&dimensions, 4., viewport);
        let anchor = layout.pages[1].origin + point(px(100.), px(100.)) + offset;
        let new_offset = layout.zoom_offset(&zoomed, offset, anchor, viewport);
        let new_position = zoomed.pages[1].origin + point(px(400.), px(400.)) + new_offset;
        assert_eq!(new_position, anchor);
        assert_eq!(zoomed.pages[1].top() - zoomed.pages[0].bottom(), px(24.));
    }
    #[::core::prelude::v1::test]
    fn fit_modes_use_the_available_viewport_and_mixed_page_dimensions() {
        let dimensions = [(500., 700.), (600., 900.)];
        for viewport in [
            size(px(700.), px(1000.)),
            size(px(500.), px(600.)),
            size(px(1200.), px(800.)),
        ] {
            let width = PreviewLayout::new(
                &dimensions,
                FitMode::Width.zoom(&dimensions, viewport, 1.),
                viewport,
            );
            assert!((f32::from(width.pages[1].size.width + px(48.) - viewport.width)).abs() < 0.01);
            let page = PreviewLayout::new(
                &dimensions,
                FitMode::Page.zoom(&dimensions, viewport, 1.),
                viewport,
            );
            for bounds in &page.pages {
                assert!(bounds.size.width + px(48.) <= viewport.width + px(0.01));
                assert!(bounds.size.height + px(48.) <= viewport.height + px(0.01));
            }
            let height_scale = (f32::from(viewport.height) - 48.) / 900.;
            assert!((page.scale - width.scale.min(height_scale)).abs() < 0.001);
        }
        assert_eq!(
            FitMode::Manual.zoom(&dimensions, size(px(700.), px(1000.)), 4.),
            4.
        );
        assert_eq!(FitMode::Width.zoom(&[], size(px(0.), px(0.)), 1.), 1.);
    }
}
