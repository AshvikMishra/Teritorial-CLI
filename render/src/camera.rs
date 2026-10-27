//! The map camera: a center point in raster coordinates plus a zoom level.
//!
//! The world wraps around horizontally, so `cx` is kept in `0..world_w`.
//! Vertically the renderer keeps the view inside the raster (or centers it when
//! the whole height fits). The renderer also tells the camera the size of the
//! last view it drew ([`Camera::note_view`]), so the camera can keep `cy` inside
//! the band the renderer can actually center on and every vertical pan moves
//! the view.

use std::cell::Cell;

use serde::{Deserialize, Serialize};
use territorial_core::{RegionId, WorldMap};

/// Zoom levels, from the whole world down to region detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ZoomLevel {
    /// The whole world width fits the view.
    World,
    /// Intermediate overview, one third of the way to Continent.
    Overview,
    /// Intermediate overview, two thirds of the way to Continent.
    Wide,
    /// 2 raster cells per screen pixel.
    Continent,
    /// Intermediate detail between Continent and Region.
    Area,
    /// Close detail between Area and Region.
    Local,
    /// 0.75 raster cells per screen pixel.
    Region,
}

impl ZoomLevel {
    /// Raster cells per screen pixel at this zoom. `World` depends on the view
    /// width, so it needs `view_px_w` (the view width in pixels) and the raster width.
    pub fn scale(self, view_px_w: u16, world_w: f32) -> f32 {
        let world = (world_w.max(1.0) / f32::from(view_px_w.max(1))).max(0.05);
        let continent = CONTINENT_SCALE.min(world);
        let region = REGION_SCALE.min(continent);
        // Geometric interpolation gives each press a similar proportional change.
        let between = |from: f32, to: f32, step: f32| from * (to / from).powf(step / 3.0);
        match self {
            ZoomLevel::World => world,
            ZoomLevel::Overview => between(world, continent, 1.0),
            ZoomLevel::Wide => between(world, continent, 2.0),
            ZoomLevel::Continent => continent,
            ZoomLevel::Area => between(continent, region, 1.0),
            ZoomLevel::Local => between(continent, region, 2.0),
            ZoomLevel::Region => region,
        }
    }

    /// A display label for the zoom level.
    pub fn label(self) -> &'static str {
        match self {
            ZoomLevel::World => "World",
            ZoomLevel::Overview => "Overview",
            ZoomLevel::Wide => "Wide",
            ZoomLevel::Continent => "Continent",
            ZoomLevel::Area => "Area",
            ZoomLevel::Local => "Local",
            ZoomLevel::Region => "Region",
        }
    }
}

/// Raster cells per pixel at [`ZoomLevel::Continent`].
pub const CONTINENT_SCALE: f32 = 2.0;
/// Raster cells per pixel at [`ZoomLevel::Region`].
pub const REGION_SCALE: f32 = 0.75;
/// Nominal view width (in pixels) used to scale panning at [`ZoomLevel::World`].
const WORLD_PAN_VIEW_W: f32 = 150.0;

/// The map camera.
///
/// Build it with [`Camera::world`]. `world_w`/`world_h` are the raster size and are
/// used to wrap and clamp the center; they are filled in by the constructor.
#[derive(Clone, Debug, PartialEq)]
pub struct Camera {
    /// Center x in raster cells (wraps around).
    pub cx: f32,
    /// Center y in raster cells.
    pub cy: f32,
    /// Current zoom.
    pub zoom: ZoomLevel,
    /// Raster width in cells.
    pub world_w: f32,
    /// Raster height in cells.
    pub world_h: f32,
    /// The last view size drawn, as `(columns, rows)` in terminal cells.
    /// `(0, 0)` means unknown, and then `cy` is only clamped to the raster.
    /// It is a `Cell` so the renderer can record it through `&Camera`.
    view: Cell<(u16, u16)>,
}

impl Camera {
    /// A camera showing the whole world, centered.
    pub fn world(map: &WorldMap) -> Camera {
        let w = f32::from(map.raster.width.max(1));
        let h = f32::from(map.raster.height.max(1));
        Camera {
            cx: w / 2.0,
            cy: h / 2.0,
            zoom: ZoomLevel::World,
            world_w: w,
            world_h: h,
            view: Cell::new((0, 0)),
        }
    }

    /// Records the size of the map view, in terminal cells, that this camera
    /// was last drawn into. The map renderer calls it on every draw. Later
    /// camera moves use it to keep `cy` where the renderer can center the view,
    /// so pans near the top and bottom edges are not swallowed.
    pub fn note_view(&self, cols: u16, rows: u16) {
        self.view.set((cols, rows));
    }

    /// Centers on region `r`'s label and zooms in to at least [`ZoomLevel::Continent`].
    /// Unknown ids are ignored.
    pub fn focus_region(&mut self, map: &WorldMap, r: RegionId) {
        self.world_w = f32::from(map.raster.width.max(1));
        self.world_h = f32::from(map.raster.height.max(1));
        if let Some(info) = map.get_region(r) {
            self.cx = f32::from(info.label.0) + 0.5;
            self.cy = f32::from(info.label.1) + 0.5;
            if self.zoom == ZoomLevel::World {
                self.zoom = ZoomLevel::Continent;
            }
            self.normalize();
        }
    }

    /// Centers on raster point `(x, y)` without changing the zoom.
    pub fn center_on(&mut self, x: f32, y: f32) {
        self.cx = x;
        self.cy = y;
        self.normalize();
    }

    /// Pans by `dx`, `dy` screen cells, scaled by the zoom (one cell is one pixel wide
    /// and two pixels tall).
    pub fn pan(&mut self, dx: i32, dy: i32) {
        // Snap to the reachable band first, so a center left outside it (for
        // example by a camera reset before the first draw) does not eat the pan.
        self.normalize();
        let s = self.pan_scale();
        self.cx += dx as f32 * s;
        self.cy += dy as f32 * s * 2.0;
        self.normalize();
    }

    /// Zooms in one of seven levels. Stays at Region.
    pub fn zoom_in(&mut self) {
        self.zoom = match self.zoom {
            ZoomLevel::World => ZoomLevel::Overview,
            ZoomLevel::Overview => ZoomLevel::Wide,
            ZoomLevel::Wide => ZoomLevel::Continent,
            ZoomLevel::Continent => ZoomLevel::Area,
            ZoomLevel::Area => ZoomLevel::Local,
            ZoomLevel::Local => ZoomLevel::Region,
            ZoomLevel::Region => ZoomLevel::Region,
        };
        self.normalize();
    }

    /// Zooms out one level. Stays at World.
    pub fn zoom_out(&mut self) {
        self.zoom = match self.zoom {
            ZoomLevel::Region => ZoomLevel::Local,
            ZoomLevel::Local => ZoomLevel::Area,
            ZoomLevel::Area => ZoomLevel::Continent,
            ZoomLevel::Continent => ZoomLevel::Wide,
            ZoomLevel::Wide => ZoomLevel::Overview,
            ZoomLevel::Overview => ZoomLevel::World,
            ZoomLevel::World => ZoomLevel::World,
        };
        self.normalize();
    }

    /// Raster cells per pixel used for panning.
    fn pan_scale(&self) -> f32 {
        let (cols, _) = self.view.get();
        self.zoom.scale(if cols == 0 { WORLD_PAN_VIEW_W as u16 } else { cols }, self.world_w)
    }

    /// Wraps `cx` into `0..world_w` and clamps `cy`.
    ///
    /// With a known view size shorter than the raster, `cy` is clamped to the
    /// band the renderer can center on, `view_h / 2 ..= world_h - view_h / 2`,
    /// so every vertical pan moves the view. The view height matches the
    /// renderer's: `rows * 2 * scale` raster rows. Otherwise (no view size yet,
    /// or the whole height fits and the renderer centers the map) `cy` is only
    /// clamped to `0..=world_h`, which keeps it for a later zoom in.
    fn normalize(&mut self) {
        let w = self.world_w.max(1.0);
        let h = self.world_h.max(1.0);
        if !self.cx.is_finite() {
            self.cx = w / 2.0;
        }
        if !self.cy.is_finite() {
            self.cy = h / 2.0;
        }
        self.cx = self.cx.rem_euclid(w);
        let (cols, rows) = self.view.get();
        let vh = if rows > 0 {
            f32::from(rows) * 2.0 * self.zoom.scale(cols, w)
        } else {
            0.0
        };
        self.cy = if vh > 0.0 && vh < h {
            self.cy.clamp(vh / 2.0, h - vh / 2.0)
        } else {
            self.cy.clamp(0.0, h)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_steps_are_gradual_reversible_and_bounded() {
        let mut camera = Camera::world(&WorldMap::builtin());
        let levels = [ZoomLevel::World, ZoomLevel::Overview, ZoomLevel::Wide,
            ZoomLevel::Continent, ZoomLevel::Area, ZoomLevel::Local, ZoomLevel::Region];
        for width in [60, 120, 240, 1000] {
            camera.zoom = ZoomLevel::World;
            let mut previous = camera.zoom.scale(width, camera.world_w);
            for &level in &levels {
                assert_eq!(camera.zoom, level);
                let scale = camera.zoom.scale(width, camera.world_w);
                assert!(scale > 0.0 && scale <= previous);
                previous = scale;
                camera.zoom_in();
            }
            assert_eq!(camera.zoom, ZoomLevel::Region);
            for &level in levels.iter().rev() {
                assert_eq!(camera.zoom, level);
                camera.zoom_out();
            }
            assert_eq!(camera.zoom, ZoomLevel::World);
        }
    }
}
