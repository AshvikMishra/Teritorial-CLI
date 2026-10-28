//! The world map renderer.
//!
//! The region raster is sampled once per (view size, camera, glyph mode) into a
//! cached grid of region ids, one per screen pixel, together with per-pixel
//! border and ocean-texture flags. Every frame only recolors that grid from the
//! current ownership (the PRD's "rasterize once, recolor only"), then draws march
//! trails, labels and march arrows on top.
//!
//! * Unicode mode: half-block `▀` cells, foreground = top pixel, background = bottom pixel.
//! * ASCII mode: one pixel per cell, `#` owned, `+` border, `.` neutral, `~` ocean texture.

use std::sync::Arc;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use territorial_core::map::OCEAN;
use territorial_core::{Game, PlayerId, Raster, RegionId, WorldMap};

use crate::camera::{Camera, ZoomLevel};
use crate::caps::{Capabilities, ColorMode, GlyphMode};
use crate::compact_number;
use crate::theme::{quantize, Rgb, Theme};

/// Pixel flag: the right or lower neighbor pixel belongs to a different region.
const FLAG_BORDER: u8 = 1;
/// Pixel flag: an ocean pixel drawn with the alternate (texture) color.
const FLAG_TEXTURE: u8 = 2;

/// Pulse period of the highlighted region, in animation frames.
const PULSE_PERIOD: u32 = 16;

/// Everything one map frame needs.
#[derive(Clone, Copy, Debug)]
pub struct MapRenderCtx<'a> {
    /// The game being shown.
    pub game: &'a Game,
    /// The camera.
    pub camera: &'a Camera,
    /// The active theme.
    pub theme: &'a Theme,
    /// Terminal capabilities (renderer and color quantization).
    pub caps: Capabilities,
    /// Whose point of view: this player's labels are bold and their marches are drawn last.
    pub viewer: PlayerId,
    /// A region to pulse (for example the target of the command being typed).
    pub highlight: Option<RegionId>,
    /// Animation frame counter; the highlight pulse cycles every 16 frames.
    pub anim_frame: u32,
    /// Draw a crosshair where the camera center lands on screen (the hotkey
    /// selection point).
    pub crosshair: bool,
}

/// What the cached grid depends on.
#[derive(Clone, Copy, Debug, PartialEq)]
struct GridKey {
    width: u16,
    height: u16,
    rows_per_cell: u16,
    cx: f32,
    cy: f32,
    zoom: ZoomLevel,
    world_w: f32,
    world_h: f32,
}

/// The cached region-id-per-pixel grid for one view.
struct Grid {
    key: GridKey,
    /// View width in pixels (= cells).
    px_w: usize,
    /// View height in pixels (= cells * rows_per_cell).
    px_h: usize,
    /// Row stride of `cells` (`px_w + 1`; one extra column and row for border tests).
    stride: usize,
    /// Region id per pixel, [`OCEAN`] for ocean. `(px_w + 1) * (px_h + 1)` entries.
    cells: Vec<u16>,
    /// Flags per visible pixel, `px_w * px_h` entries.
    flags: Vec<u8>,
    /// Visible pixels per region.
    region_pixels: Vec<u32>,
    /// Raster coordinates of the view's top-left corner.
    origin_x: f32,
    origin_y: f32,
    /// Raster cells per pixel, horizontally and vertically.
    scale_x: f32,
    scale_y: f32,
    /// Raster width (for horizontal wrap).
    world_w: f32,
}

impl Grid {
    fn region(&self, px: usize, py: usize) -> u16 {
        self.cells[py * self.stride + px]
    }

    fn flags(&self, px: usize, py: usize) -> u8 {
        self.flags[py * self.px_w + px]
    }

    /// Raster point to fractional pixel coordinates, or `None` when outside the view.
    fn to_pixel(&self, rx: f32, ry: f32) -> Option<(f32, f32)> {
        let dx = (rx - self.origin_x).rem_euclid(self.world_w);
        let fx = dx / self.scale_x;
        let fy = (ry - self.origin_y) / self.scale_y;
        if !fx.is_finite() || !fy.is_finite() {
            return None;
        }
        if fx < 0.0 || fy < 0.0 || fx >= self.px_w as f32 || fy >= self.px_h as f32 {
            return None;
        }
        Some((fx, fy))
    }
}

/// Per-frame colors.
struct FramePalette {
    /// Fill color per region.
    fill: Vec<Color>,
    /// Border color per region.
    edge: Vec<Color>,
    /// Fill color per region before quantization (for labels).
    fill_rgb: Vec<Rgb>,
    /// Whether each region has an owner.
    owned: Vec<bool>,
    ocean: Color,
    ocean_alt: Color,
    /// Foreground of `~` ocean texture in ASCII mode.
    ocean_glyph: Color,
}

/// Renders the world map into a ratatui buffer. Keep one per map and reuse it
/// across frames so the pixel grid cache stays warm.
pub struct MapRenderer {
    map: Arc<WorldMap>,
    grid: Option<Grid>,
}

impl MapRenderer {
    /// Creates a renderer for `map`.
    pub fn new(map: Arc<WorldMap>) -> MapRenderer {
        MapRenderer { map, grid: None }
    }

    /// The map this renderer draws.
    pub fn map(&self) -> &Arc<WorldMap> {
        &self.map
    }

    /// Drops the cached grid so the next frame resamples the raster.
    pub fn invalidate(&mut self) {
        self.grid = None;
    }

    /// The region under screen cell `(x, y)` (absolute buffer coordinates) in the
    /// last rendered frame drawn into `area`. In Unicode mode the cell's top pixel is used.
    pub fn region_at_cell(&self, area: Rect, x: u16, y: u16) -> Option<RegionId> {
        let grid = self.grid.as_ref()?;
        if x < area.x || y < area.y {
            return None;
        }
        let px = usize::from(x - area.x);
        let py = usize::from(y - area.y) * usize::from(grid.key.rows_per_cell);
        if px >= grid.px_w || py >= grid.px_h {
            return None;
        }
        let id = grid.region(px, py);
        if id == OCEAN {
            None
        } else {
            Some(id)
        }
    }

    /// Draws the map into `area` of `buf`. Everything is clipped to both `area`
    /// and the buffer, so any `area` is safe.
    pub fn render(&mut self, ctx: &MapRenderCtx<'_>, area: Rect, buf: &mut Buffer) {
        let area = area.intersection(*buf.area());
        if area.width == 0 || area.height == 0 {
            return;
        }
        // Let the camera clamp its center to what this view can show.
        ctx.camera.note_view(area.width, area.height);
        let unicode = ctx.caps.glyphs == GlyphMode::Unicode;
        let rows_per_cell: u16 = if unicode { 2 } else { 1 };
        let key = GridKey {
            width: area.width,
            height: area.height,
            rows_per_cell,
            cx: ctx.camera.cx,
            cy: ctx.camera.cy,
            zoom: ctx.camera.zoom,
            world_w: f32::from(self.map.raster.width.max(1)),
            world_h: f32::from(self.map.raster.height.max(1)),
        };
        let stale = match &self.grid {
            Some(g) => g.key != key,
            None => true,
        };
        if stale {
            self.grid = Some(build_grid(&self.map, key));
        }
        let grid = match self.grid.as_ref() {
            Some(g) => g,
            None => return,
        };
        let map: &WorldMap = &self.map;
        let pal = build_palette(ctx, map.len());

        if unicode {
            draw_half_blocks(grid, &pal, area, buf);
        } else {
            draw_ascii(grid, &pal, ctx, area, buf);
        }
        draw_march_trails(grid, map, ctx, area, buf);
        draw_labels(grid, map, &pal, ctx, area, buf);
        draw_march_arrows(grid, map, ctx, area, buf);
        if ctx.crosshair {
            draw_crosshair(grid, ctx, area, buf);
        }
    }
}

/// Samples the raster into a grid for `key`.
fn build_grid(map: &WorldMap, key: GridKey) -> Grid {
    let px_w = usize::from(key.width);
    let px_h = usize::from(key.height) * usize::from(key.rows_per_cell.max(1));
    let world_w = key.world_w.max(1.0);
    let world_h = key.world_h.max(1.0);
    let scale_x = key.zoom.scale(key.width, world_w);
    let scale_y = scale_x * 2.0 / f32::from(key.rows_per_cell.max(1));

    let cx = if key.cx.is_finite() { key.cx } else { world_w / 2.0 };
    let cy = if key.cy.is_finite() { key.cy } else { world_h / 2.0 };
    let view_w = px_w as f32 * scale_x;
    let view_h = px_h as f32 * scale_y;
    let origin_x = cx - view_w / 2.0;
    let origin_y = if view_h >= world_h {
        (world_h - view_h) / 2.0
    } else {
        let mut oy = cy - view_h / 2.0;
        if oy > world_h - view_h {
            oy = world_h - view_h;
        }
        if oy < 0.0 {
            oy = 0.0;
        }
        oy
    };

    let stride = px_w + 1;
    let rows = px_h + 1;
    let supersample = scale_x > 1.5;
    let off_x = scale_x * 0.25;
    let off_y = scale_y * 0.25;
    let mut cells = vec![OCEAN; stride * rows];
    for py in 0..rows {
        let ry = origin_y + (py as f32 + 0.5) * scale_y;
        for px in 0..stride {
            let rx = origin_x + (px as f32 + 0.5) * scale_x;
            cells[py * stride + px] = sample(&map.raster, rx, ry, supersample, off_x, off_y);
        }
    }

    let mut flags = vec![0u8; px_w * px_h];
    let mut region_pixels = vec![0u32; map.len()];
    for py in 0..px_h {
        for px in 0..px_w {
            let id = cells[py * stride + px];
            let mut f = 0u8;
            if id == OCEAN {
                let rx = origin_x + (px as f32 + 0.5) * scale_x;
                let ry = origin_y + (py as f32 + 0.5) * scale_y;
                let wx = (rx.rem_euclid(world_w) / scale_x).floor() as i32;
                let wy = (ry / scale_y).floor() as i32;
                if ocean_texture(wx, wy) {
                    f |= FLAG_TEXTURE;
                }
            } else {
                let right = cells[py * stride + px + 1];
                let below = cells[(py + 1) * stride + px];
                if right != id || below != id {
                    f |= FLAG_BORDER;
                }
                if let Some(c) = region_pixels.get_mut(usize::from(id)) {
                    *c += 1;
                }
            }
            flags[py * px_w + px] = f;
        }
    }

    Grid {
        key,
        px_w,
        px_h,
        stride,
        cells,
        flags,
        region_pixels,
        origin_x,
        origin_y,
        scale_x,
        scale_y,
        world_w,
    }
}

/// The region at raster point `(rx, ry)`. When zoomed out, ocean hits are
/// re-checked at four sub-points so small islands stay visible.
fn sample(raster: &Raster, rx: f32, ry: f32, supersample: bool, off_x: f32, off_y: f32) -> u16 {
    if let Some(r) = raster.region_at(rx.floor() as i32, ry.floor() as i32) {
        return r;
    }
    if supersample {
        let offsets = [
            (-off_x, -off_y),
            (off_x, -off_y),
            (-off_x, off_y),
            (off_x, off_y),
        ];
        for (ox, oy) in offsets.iter() {
            let x = (rx + *ox).floor() as i32;
            let y = (ry + *oy).floor() as i32;
            if let Some(r) = raster.region_at(x, y) {
                return r;
            }
        }
    }
    OCEAN
}

/// A sparse, stable speckle of short horizontal dashes for the ocean.
fn ocean_texture(wx: i32, wy: i32) -> bool {
    let a = ((wx >> 1) as u32).wrapping_mul(0x9E37_79B1);
    let b = (wy as u32).wrapping_mul(0x85EB_CA77);
    let mut h = a ^ b;
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h % 13 == 0
}

/// Pulse strength in percent (10..=66) for the highlight at `frame`.
fn pulse_pct(frame: u32) -> u8 {
    let half = PULSE_PERIOD / 2;
    let t = frame % PULSE_PERIOD;
    let tri = if t < half { t } else { PULSE_PERIOD - t };
    (10 + tri * 7).min(100) as u8
}

/// Whether the on/off pulse (for 16-color and ASCII modes) is in its "on" half.
fn pulse_on(frame: u32) -> bool {
    (frame % PULSE_PERIOD) < PULSE_PERIOD / 2
}

fn build_palette(ctx: &MapRenderCtx<'_>, n: usize) -> FramePalette {
    let mode = ctx.caps.color;
    let theme = ctx.theme;
    let regions = ctx.game.regions();
    let mut fill = Vec::with_capacity(n);
    let mut edge = Vec::with_capacity(n);
    let mut fill_rgb = Vec::with_capacity(n);
    let mut owned = Vec::with_capacity(n);
    for i in 0..n {
        let owner = match regions.get(i) {
            Some(st) => st.owner,
            None => None,
        };
        let base = theme.owner_color(owner);
        let is_hl = ctx.highlight.map(usize::from) == Some(i);
        let (rgb, f, e) = if mode == ColorMode::Ansi16 {
            let rgb = if is_hl && pulse_on(ctx.anim_frame) {
                theme.pulse_target(base)
            } else {
                base
            };
            if owner.is_none() && !(is_hl && pulse_on(ctx.anim_frame)) {
                (rgb, Color::DarkGray, Color::Black)
            } else {
                (rgb, quantize(rgb, mode), quantize(theme.border_of(rgb), mode))
            }
        } else {
            let rgb = if is_hl {
                base.mix(theme.pulse_target(base), pulse_pct(ctx.anim_frame))
            } else {
                base
            };
            (rgb, quantize(rgb, mode), quantize(theme.border_of(rgb), mode))
        };
        fill.push(f);
        edge.push(e);
        fill_rgb.push(rgb);
        owned.push(owner.is_some());
    }
    let (ocean, ocean_alt, ocean_glyph) = if mode == ColorMode::Ansi16 {
        (Color::Black, Color::Black, Color::Blue)
    } else {
        (
            quantize(theme.ocean(), mode),
            quantize(theme.ocean_alt(), mode),
            quantize(theme.ocean_alt().lighten(25), mode),
        )
    };
    FramePalette {
        fill,
        edge,
        fill_rgb,
        owned,
        ocean,
        ocean_alt,
        ocean_glyph,
    }
}

/// The color of pixel `(px, py)`.
fn pixel_color(grid: &Grid, pal: &FramePalette, px: usize, py: usize) -> Color {
    let id = grid.region(px, py);
    let f = grid.flags(px, py);
    if id == OCEAN {
        if f & FLAG_TEXTURE != 0 {
            pal.ocean_alt
        } else {
            pal.ocean
        }
    } else {
        let i = usize::from(id);
        if i >= pal.fill.len() {
            pal.ocean
        } else if f & FLAG_BORDER != 0 {
            pal.edge[i]
        } else {
            pal.fill[i]
        }
    }
}

fn draw_half_blocks(grid: &Grid, pal: &FramePalette, area: Rect, buf: &mut Buffer) {
    for row in 0..area.height {
        let top_py = usize::from(row) * 2;
        let bot_py = top_py + 1;
        if bot_py >= grid.px_h {
            break;
        }
        for col in 0..area.width {
            let px = usize::from(col);
            if px >= grid.px_w {
                break;
            }
            let top = pixel_color(grid, pal, px, top_py);
            let bottom = pixel_color(grid, pal, px, bot_py);
            if let Some(cell) = buf.cell_mut((area.x + col, area.y + row)) {
                cell.set_symbol("\u{2580}");
                cell.set_fg(top);
                cell.set_bg(bottom);
                cell.modifier = Modifier::empty();
            }
        }
    }
}

fn draw_ascii(grid: &Grid, pal: &FramePalette, ctx: &MapRenderCtx<'_>, area: Rect, buf: &mut Buffer) {
    let hl = ctx.highlight.map(usize::from);
    let hl_on = pulse_on(ctx.anim_frame);
    let hl_color = quantize(ctx.theme.highlight(), ctx.caps.color);
    for row in 0..area.height {
        let py = usize::from(row);
        if py >= grid.px_h {
            break;
        }
        for col in 0..area.width {
            let px = usize::from(col);
            if px >= grid.px_w {
                break;
            }
            let id = grid.region(px, py);
            let f = grid.flags(px, py);
            let (ch, fg, modifier) = if id == OCEAN {
                if f & FLAG_TEXTURE != 0 {
                    ('~', pal.ocean_glyph, Modifier::empty())
                } else {
                    (' ', pal.ocean_glyph, Modifier::empty())
                }
            } else {
                let i = usize::from(id);
                if i >= pal.fill.len() {
                    (' ', pal.ocean_glyph, Modifier::empty())
                } else if hl == Some(i) && hl_on {
                    ('@', hl_color, Modifier::BOLD)
                } else if f & FLAG_BORDER != 0 {
                    ('+', pal.fill[i], Modifier::empty())
                } else if pal.owned[i] {
                    ('#', pal.fill[i], Modifier::empty())
                } else {
                    ('.', pal.fill[i], Modifier::empty())
                }
            };
            if let Some(cell) = buf.cell_mut((area.x + col, area.y + row)) {
                cell.set_char(ch);
                cell.set_fg(fg);
                cell.set_bg(Color::Reset);
                cell.modifier = modifier;
            }
        }
    }
}

/// Fractional progress (0.0..=1.0) of a march at the current tick.
fn march_progress(game: &Game, depart: u64, arrive: u64) -> f32 {
    let span = arrive.saturating_sub(depart).max(1);
    let done = game.tick().saturating_sub(depart).min(span);
    done as f32 / span as f32
}

/// Raster label point of `r` (cell center), if the region exists.
fn label_point(map: &WorldMap, r: RegionId) -> Option<(f32, f32)> {
    map.get_region(r)
        .map(|info| (f32::from(info.label.0) + 0.5, f32::from(info.label.1) + 0.5))
}

/// The march vector from `from` to `to`, taking the short way around the world.
fn march_vector(grid: &Grid, from: (f32, f32), to: (f32, f32)) -> (f32, f32) {
    let w = grid.world_w;
    let mut vx = to.0 - from.0;
    if vx > w / 2.0 {
        vx -= w;
    } else if vx < -w / 2.0 {
        vx += w;
    }
    (vx, to.1 - from.1)
}

/// Screen cell (absolute) of raster point `(rx, ry)`, if visible.
fn cell_of(grid: &Grid, area: Rect, rx: f32, ry: f32) -> Option<(u16, u16)> {
    let (fx, fy) = grid.to_pixel(rx, ry)?;
    let col = fx as usize;
    let row = (fy as usize) / usize::from(grid.key.rows_per_cell.max(1));
    if col >= usize::from(area.width) || row >= usize::from(area.height) {
        return None;
    }
    Some((area.x + col as u16, area.y + row as u16))
}

/// Dotted path ahead of marches that involve the viewer (their own, or aimed at them).
fn draw_march_trails(grid: &Grid, map: &WorldMap, ctx: &MapRenderCtx<'_>, area: Rect, buf: &mut Buffer) {
    let unicode = ctx.caps.glyphs == GlyphMode::Unicode;
    let dot = if unicode { "\u{00B7}" } else { "." };
    let regions = ctx.game.regions();
    for m in ctx.game.marches() {
        let target_owner = match regions.get(usize::from(m.to)) {
            Some(st) => st.owner,
            None => None,
        };
        let involved = m.owner == ctx.viewer || target_owner == Some(ctx.viewer);
        if !involved {
            continue;
        }
        let (from, to) = match (label_point(map, m.from), label_point(map, m.to)) {
            (Some(a), Some(b)) => (a, b),
            _ => continue,
        };
        let v = march_vector(grid, from, to);
        let progress = march_progress(ctx.game, m.depart_tick, m.arrive_tick);
        // One step per screen cell along the longer axis.
        let cells_x = (v.0 / grid.scale_x).abs();
        let cells_y = (v.1 / (grid.scale_y * f32::from(grid.key.rows_per_cell))).abs();
        let steps = cells_x.max(cells_y).ceil();
        if !steps.is_finite() || steps < 2.0 {
            continue;
        }
        let steps = steps.min(400.0) as u32;
        let color = quantize(ctx.theme.player_color(m.owner), ctx.caps.color);
        for i in 1..steps {
            let t = i as f32 / steps as f32;
            if t <= progress {
                continue;
            }
            let rx = from.0 + v.0 * t;
            let ry = from.1 + v.1 * t;
            if let Some((x, y)) = cell_of(grid, area, rx, ry) {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_symbol(dot);
                    cell.set_fg(color);
                    cell.modifier = Modifier::empty();
                    if !unicode {
                        cell.set_bg(Color::Reset);
                    }
                }
            }
        }
    }
}

/// The arrow glyph for direction `(dx, dy)` (y grows downward).
fn arrow_glyph(dx: f32, dy: f32, unicode: bool) -> &'static str {
    if dx == 0.0 && dy == 0.0 {
        return "*";
    }
    let angle = dy.atan2(dx);
    if unicode {
        let octant = ((angle / std::f32::consts::FRAC_PI_4).round() as i32).rem_euclid(8);
        match octant {
            0 => "\u{2192}", // →
            1 => "\u{2198}", // ↘
            2 => "\u{2193}", // ↓
            3 => "\u{2199}", // ↙
            4 => "\u{2190}", // ←
            5 => "\u{2196}", // ↖
            6 => "\u{2191}", // ↑
            _ => "\u{2197}", // ↗
        }
    } else {
        let quadrant = ((angle / std::f32::consts::FRAC_PI_2).round() as i32).rem_euclid(4);
        match quadrant {
            0 => ">",
            1 => "v",
            2 => "<",
            _ => "^",
        }
    }
}

/// March arrows, interpolated between source and target labels by progress.
/// The viewer's marches are drawn last so they stay on top.
fn draw_march_arrows(grid: &Grid, map: &WorldMap, ctx: &MapRenderCtx<'_>, area: Rect, buf: &mut Buffer) {
    let unicode = ctx.caps.glyphs == GlyphMode::Unicode;
    let bg = if unicode {
        quantize(ctx.theme.panel_bg(), ctx.caps.color)
    } else {
        Color::Reset
    };
    for pass in 0..2 {
        for m in ctx.game.marches() {
            let mine = m.owner == ctx.viewer;
            if (pass == 0) == mine {
                continue;
            }
            let (from, to) = match (label_point(map, m.from), label_point(map, m.to)) {
                (Some(a), Some(b)) => (a, b),
                _ => continue,
            };
            let v = march_vector(grid, from, to);
            let progress = march_progress(ctx.game, m.depart_tick, m.arrive_tick);
            let rx = from.0 + v.0 * progress;
            let ry = from.1 + v.1 * progress;
            let (x, y) = match cell_of(grid, area, rx, ry) {
                Some(p) => p,
                None => continue,
            };
            let rgb = ctx.theme.player_color(m.owner).lighten(15);
            let fg = quantize(rgb, ctx.caps.color);
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol(arrow_glyph(v.0, v.1, unicode));
                cell.set_fg(fg);
                cell.set_bg(bg);
                cell.modifier = Modifier::BOLD;
            }
        }
    }
}

/// The crosshair at the camera center. It is drawn where `(cx, cy)` actually
/// lands, which is not the middle of the view when the whole map height fits
/// and the renderer centers the map instead.
fn draw_crosshair(grid: &Grid, ctx: &MapRenderCtx<'_>, area: Rect, buf: &mut Buffer) {
    let (x, y) = match cell_of(grid, area, ctx.camera.cx, ctx.camera.cy) {
        Some(p) => p,
        None => return,
    };
    let fg = quantize(ctx.theme.highlight(), ctx.caps.color);
    if let Some(cell) = buf.cell_mut((x, y)) {
        if ctx.caps.glyphs == GlyphMode::Unicode {
            cell.set_symbol("\u{271B}"); // ✛
            cell.set_fg(fg);
            cell.set_bg(quantize(ctx.theme.panel_bg(), ctx.caps.color));
            cell.modifier = Modifier::BOLD;
        } else {
            // Reversed, so it stands apart from the `+` border glyphs.
            cell.set_char('+');
            cell.set_fg(fg);
            cell.set_bg(Color::Reset);
            cell.modifier = Modifier::BOLD | Modifier::REVERSED;
        }
    }
}

/// Label text for region `r`: `*` for a capital, the code, then optionally the
/// garrison and `+` when fortified.
fn label_text(game: &Game, map: &WorldMap, r: RegionId, with_garrison: bool) -> String {
    let mut s = String::new();
    let state = game.regions().get(usize::from(r));
    if let Some(st) = state {
        if let Some(owner) = st.owner {
            let capital = game.get_player(owner).and_then(|p| p.capital);
            if capital == Some(r) {
                s.push('*');
            }
        }
    }
    if let Some(info) = map.get_region(r) {
        s.push_str(&info.code);
    }
    if with_garrison {
        if let Some(st) = state {
            s.push(' ');
            s.push_str(&compact_number(st.garrison));
            if game.tick() < st.fortified_until {
                s.push('+');
            }
        }
    }
    s
}

/// Region labels, placed greedily by priority without overlapping.
fn draw_labels(
    grid: &Grid,
    map: &WorldMap,
    pal: &FramePalette,
    ctx: &MapRenderCtx<'_>,
    area: Rect,
    buf: &mut Buffer,
) {
    let w = usize::from(area.width);
    let h = usize::from(area.height);
    let roomy = area.width >= 24 && area.height >= 6;
    let zoomed = ctx.camera.zoom != ZoomLevel::World;
    let rows_per_cell = usize::from(grid.key.rows_per_cell.max(1));
    let regions = ctx.game.regions();

    // (class, visible pixels, region, text, anchor column, anchor row)
    let mut candidates: Vec<(u8, u32, RegionId, String, usize, usize)> = Vec::new();
    for (i, info) in map.regions.iter().enumerate() {
        let r = i as RegionId;
        let is_hl = ctx.highlight == Some(r);
        if !roomy && !is_hl {
            continue;
        }
        let anchor = grid.to_pixel(f32::from(info.label.0) + 0.5, f32::from(info.label.1) + 0.5);
        let (fx, fy) = match anchor {
            Some(p) => p,
            None => continue,
        };
        let pixels = grid.region_pixels.get(i).copied().unwrap_or(0);
        let visible_cells = (pixels as usize) / rows_per_cell;
        let full = label_text(ctx.game, map, r, true);
        let text = if zoomed || is_hl || visible_cells >= full.len() * 3 {
            full
        } else {
            let short = label_text(ctx.game, map, r, false);
            if visible_cells >= short.len() * 4 {
                short
            } else {
                continue;
            }
        };
        if text.is_empty() || text.len() > w {
            continue;
        }
        let owner = match regions.get(i) {
            Some(st) => st.owner,
            None => None,
        };
        let class = if is_hl {
            0
        } else if owner == Some(ctx.viewer) {
            1
        } else if owner.is_some() {
            2
        } else {
            3
        };
        let col = fx as usize;
        let row = (fy as usize) / rows_per_cell;
        if row >= h {
            continue;
        }
        candidates.push((class, pixels, r, text, col, row));
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));

    let mut occupied = vec![false; w * h];
    for (_, _, r, text, col, row) in candidates.iter() {
        let len = text.len();
        let mut start = col.saturating_sub(len / 2);
        if start + len > w {
            start = w - len;
        }
        let lo = start.saturating_sub(1);
        let hi = (start + len + 1).min(w);
        let row_base = row * w;
        if occupied[row_base + lo..row_base + hi].iter().any(|o| *o) {
            continue;
        }
        for o in occupied[row_base + lo..row_base + hi].iter_mut() {
            *o = true;
        }
        let i = usize::from(*r);
        let bg_rgb = pal.fill_rgb.get(i).copied().unwrap_or_else(|| ctx.theme.neutral());
        let fg = quantize(ctx.theme.text_on(bg_rgb), ctx.caps.color);
        let bg = match pal.fill.get(i) {
            Some(c) => *c,
            None => Color::Reset,
        };
        let owner = match regions.get(i) {
            Some(st) => st.owner,
            None => None,
        };
        let modifier = if owner == Some(ctx.viewer) || ctx.highlight == Some(*r) {
            Modifier::BOLD
        } else {
            Modifier::empty()
        };
        let y = area.y + *row as u16;
        for (k, ch) in text.chars().enumerate() {
            let x = area.x + (start + k) as u16;
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_char(ch);
                cell.set_fg(fg);
                cell.set_bg(bg);
                cell.modifier = modifier;
            }
        }
    }
}
