//! The map camera: a center point in raster coordinates plus a zoom level.
//!
//! The world wraps around horizontally, so `cx` is kept in `0..world_w`.
//! Vertically the renderer keeps the view inside the raster (or centers it when
//! the whole height fits).

use serde::{Deserialize, Serialize};
use territorial_core::{RegionId, WorldMap};

/// Zoom levels, from the whole world down to region detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ZoomLevel {
    /// The whole world width fits the view.
    World,
    /// 2 raster cells per screen pixel.
    Continent,
    /// 0.75 raster cells per screen pixel.
    Region,
}

impl ZoomLevel {
    /// Raster cells per screen pixel at this zoom. `World` depends on the view
    /// width, so it needs `view_px_w` (the view width in pixels) and the raster width.
    pub fn scale(self, view_px_w: u16, world_w: f32) -> f32 {
        match self {
            ZoomLevel::World => {
                let w = f32::from(view_px_w.max(1));
                (world_w.max(1.0) / w).max(0.05)
            }
            ZoomLevel::Continent => CONTINENT_SCALE,
            ZoomLevel::Region => REGION_SCALE,
        }
    }

    /// A display label (`World`, `Continent`, `Region`).
    pub fn label(self) -> &'static str {
        match self {
            ZoomLevel::World => "World",
            ZoomLevel::Continent => "Continent",
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
#[derive(Clone, Copy, Debug, PartialEq)]
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
        }
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
        let s = self.pan_scale();
        self.cx += dx as f32 * s;
        self.cy += dy as f32 * s * 2.0;
        self.normalize();
    }

    /// Zooms in one level (World, Continent, Region). Stays at Region.
    pub fn zoom_in(&mut self) {
        self.zoom = match self.zoom {
            ZoomLevel::World => ZoomLevel::Continent,
            ZoomLevel::Continent => ZoomLevel::Region,
            ZoomLevel::Region => ZoomLevel::Region,
        };
    }

    /// Zooms out one level. Stays at World.
    pub fn zoom_out(&mut self) {
        self.zoom = match self.zoom {
            ZoomLevel::Region => ZoomLevel::Continent,
            ZoomLevel::Continent => ZoomLevel::World,
            ZoomLevel::World => ZoomLevel::World,
        };
        self.normalize();
    }

    /// Raster cells per pixel used for panning.
    fn pan_scale(&self) -> f32 {
        match self.zoom {
            ZoomLevel::World => (self.world_w.max(1.0) / WORLD_PAN_VIEW_W).max(1.0),
            ZoomLevel::Continent => CONTINENT_SCALE,
            ZoomLevel::Region => REGION_SCALE,
        }
    }

    /// Wraps `cx` into `0..world_w` and clamps `cy` into `0..=world_h`.
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
        self.cy = self.cy.clamp(0.0, h);
    }
}
