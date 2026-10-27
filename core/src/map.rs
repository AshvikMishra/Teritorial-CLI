//! The world map: a graph of regions joined by land borders and sea lanes, plus a
//! raster that says which region covers each map cell.
//!
//! The map is produced by `mapgen` (see `docs/ARCHITECTURE.md` section 2) as two
//! files: `world_regions.json` (regions, edges and raster metadata) and
//! `world_bitmap.bin` (a run-length encoded region-id raster). [`WorldMap::load`]
//! parses and validates both without panicking on bad input, and
//! [`WorldMap::builtin`] returns the copy embedded in the binary.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};

use crate::RegionId;

/// Raster value for ocean, or a cell that belongs to no region.
pub const OCEAN: u16 = 0xFFFF;

/// Whether an edge is a shared land border or a curated sea lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeKind {
    /// A shared land border.
    Land,
    /// A sea lane between coasts or islands.
    Sea,
}

/// An undirected edge of the region graph. Always stored with `a < b`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    /// The lower region id.
    pub a: RegionId,
    /// The higher region id.
    pub b: RegionId,
    /// Land border or sea lane.
    pub kind: EdgeKind,
    /// Great-circle distance between the two region labels, in kilometres (at least 1).
    pub length_km: u32,
}

/// Static information about one region.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionInfo {
    /// Region id (index into [`WorldMap::regions`]).
    pub id: RegionId,
    /// Short unique lowercase alias, e.g. `fr` or `us-w`.
    pub code: String,
    /// Display name, e.g. `France`.
    pub name: String,
    /// Continent name.
    pub continent: String,
    /// Economic weight (1..=30). Drives troop growth, caps and score.
    pub value: u32,
    /// Area in square kilometres.
    pub area_km2: u64,
    /// Population.
    pub population: u64,
    /// Raster coordinates `(x, y)` of a cell inside the region, near its visual center.
    pub label: (u16, u16),
    /// Number of raster cells covered by the region (counted from the bitmap).
    pub cells: u32,
}

/// The region raster. Cell `(x, y)` holds a region id or [`OCEAN`]; row 0 is the north edge.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Raster {
    /// Width in cells.
    pub width: u16,
    /// Height in cells.
    pub height: u16,
    /// Cells per degree of latitude/longitude.
    pub cells_per_degree: u16,
    /// Latitude of the top edge, in degrees.
    pub lat_top: i16,
    /// Longitude of the left edge, in degrees.
    pub lon_left: i16,
    cells: Vec<u16>,
}

impl Raster {
    /// The region at cell `(x, y)`, or `None` for ocean or when `y` is out of bounds.
    /// `x` wraps around horizontally (the world is a cylinder).
    pub fn region_at(&self, x: i32, y: i32) -> Option<RegionId> {
        if self.width == 0 || y < 0 || y >= self.height as i32 {
            return None;
        }
        let w = self.width as i32;
        let xx = x.rem_euclid(w);
        let idx = (y as usize) * (self.width as usize) + (xx as usize);
        match self.cells.get(idx) {
            Some(&v) if v != OCEAN => Some(v),
            _ => None,
        }
    }

    /// All cells in row-major order (`width * height` entries, [`OCEAN`] for no region).
    pub fn cells(&self) -> &[u16] {
        &self.cells
    }
}

/// Error returned by [`WorldMap::load`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MapError {
    /// The regions JSON could not be parsed.
    Json(String),
    /// The bitmap is truncated or malformed.
    Bitmap(String),
    /// The data parsed but violates an invariant.
    Invalid(String),
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MapError::Json(m) => write!(f, "invalid world regions JSON: {}", m),
            MapError::Bitmap(m) => write!(f, "invalid world bitmap: {}", m),
            MapError::Invalid(m) => write!(f, "invalid world map: {}", m),
        }
    }
}

impl std::error::Error for MapError {}

/// The world: regions, edges, raster and a precomputed adjacency list.
///
/// `WorldMap::default()` is an empty map; it is only used as a placeholder
/// while a deserialized [`crate::Game`] waits for [`crate::Game::attach_map`].
#[derive(Clone, Debug, Default)]
pub struct WorldMap {
    /// Regions, indexed by [`RegionId`].
    pub regions: Vec<RegionInfo>,
    /// Edges sorted by `(a, b)`, with `a < b` and no duplicates.
    pub edges: Vec<Edge>,
    /// The region raster.
    pub raster: Raster,
    adjacency: Vec<Vec<(RegionId, u32, EdgeKind)>>,
    code_index: BTreeMap<String, RegionId>,
    total_value: u64,
}

static BUILTIN_MAP: OnceLock<Arc<WorldMap>> = OnceLock::new();

impl WorldMap {
    /// Parses and validates a map from the mapgen JSON and RLE bitmap.
    ///
    /// Checks: ids are contiguous, codes are unique, edges reference valid
    /// regions, the bitmap is well formed and matches the JSON raster size,
    /// every region covers at least one cell and the graph is connected.
    pub fn load(regions_json: &str, bitmap: &[u8]) -> Result<WorldMap, MapError> {
        let file: RegionsFile =
            serde_json::from_str(regions_json).map_err(|e| MapError::Json(e.to_string()))?;
        let version = num_to_i64(&file.version, "version")?;
        if version != 1 {
            return Err(MapError::Invalid(format!(
                "unsupported regions file version {}",
                version
            )));
        }

        // Raster metadata.
        let width = num_in_range(&file.raster.width, "raster.width", 1, u16::MAX as i64)? as u16;
        let height =
            num_in_range(&file.raster.height, "raster.height", 1, u16::MAX as i64)? as u16;
        let cpd = num_in_range(
            &file.raster.cells_per_degree,
            "raster.cells_per_degree",
            1,
            u16::MAX as i64,
        )? as u16;
        let lat_top = num_in_range(
            &file.raster.lat_top,
            "raster.lat_top",
            i16::MIN as i64,
            i16::MAX as i64,
        )? as i16;
        let lon_left = num_in_range(
            &file.raster.lon_left,
            "raster.lon_left",
            i16::MIN as i64,
            i16::MAX as i64,
        )? as i16;

        // Regions.
        let count = file.regions.len();
        if count == 0 {
            return Err(MapError::Invalid("the map has no regions".to_string()));
        }
        if count >= OCEAN as usize {
            return Err(MapError::Invalid(format!("too many regions ({})", count)));
        }
        let mut indexed: Vec<(usize, RegionInfo)> = Vec::with_capacity(count);
        for (pos, rj) in file.regions.iter().enumerate() {
            let what = format!("regions[{}]", pos);
            let id = num_in_range(&rj.id, &format!("{}.id", what), 0, count as i64 - 1)? as usize;
            let code = rj.code.trim().to_ascii_lowercase();
            if code.is_empty() {
                return Err(MapError::Invalid(format!("{} has an empty code", what)));
            }
            let value = num_in_range(&rj.value, &format!("{}.value", what), 0, 1_000_000)?;
            let value = value.max(1) as u32;
            let area = match &rj.area_km2 {
                Some(n) => num_to_i64(n, "area_km2")?.max(0) as u64,
                None => 0,
            };
            let population = match &rj.population {
                Some(n) => num_to_i64(n, "population")?.max(0) as u64,
                None => 0,
            };
            if rj.label.len() < 2 {
                return Err(MapError::Invalid(format!("{} label needs [x, y]", what)));
            }
            let lx = num_to_i64(&rj.label[0], "label x")?.clamp(0, width as i64 - 1) as u16;
            let ly = num_to_i64(&rj.label[1], "label y")?.clamp(0, height as i64 - 1) as u16;
            indexed.push((
                id,
                RegionInfo {
                    id: id as RegionId,
                    code,
                    name: rj.name.clone(),
                    continent: rj.continent.clone(),
                    value,
                    area_km2: area,
                    population,
                    label: (lx, ly),
                    cells: 0,
                },
            ));
        }
        indexed.sort_by_key(|(id, _)| *id);
        let mut regions: Vec<RegionInfo> = Vec::with_capacity(count);
        for (expected, (id, info)) in indexed.into_iter().enumerate() {
            if id != expected {
                return Err(MapError::Invalid(format!(
                    "region ids are not contiguous (missing or duplicate id near {})",
                    expected
                )));
            }
            regions.push(info);
        }
        let mut code_index: BTreeMap<String, RegionId> = BTreeMap::new();
        for info in &regions {
            if code_index.insert(info.code.clone(), info.id).is_some() {
                return Err(MapError::Invalid(format!("duplicate region code '{}'", info.code)));
            }
        }

        // Edges.
        let mut edges: Vec<Edge> = Vec::with_capacity(file.edges.len());
        for (pos, ej) in file.edges.iter().enumerate() {
            let what = format!("edges[{}]", pos);
            let a = num_in_range(&ej.a, &format!("{}.a", what), 0, count as i64 - 1)? as RegionId;
            let b = num_in_range(&ej.b, &format!("{}.b", what), 0, count as i64 - 1)? as RegionId;
            if a == b {
                return Err(MapError::Invalid(format!("{} is a self loop", what)));
            }
            let length = num_to_i64(&ej.length_km, "length_km")?.clamp(1, u32::MAX as i64) as u32;
            let (lo, hi) = if a < b { (a, b) } else { (b, a) };
            edges.push(Edge {
                a: lo,
                b: hi,
                kind: ej.kind,
                length_km: length,
            });
        }
        edges.sort_by(|x, y| (x.a, x.b).cmp(&(y.a, y.b)));
        edges.dedup_by(|x, y| x.a == y.a && x.b == y.b);

        // Bitmap.
        let cells = decode_bitmap(bitmap, width, height, count)?;
        let mut counts: Vec<u32> = vec![0; count];
        for &c in &cells {
            if c != OCEAN {
                counts[c as usize] = counts[c as usize].saturating_add(1);
            }
        }
        for info in regions.iter_mut() {
            info.cells = counts[info.id as usize];
            if info.cells == 0 {
                return Err(MapError::Invalid(format!(
                    "region '{}' covers no raster cells",
                    info.code
                )));
            }
        }

        // Adjacency.
        let mut adjacency: Vec<Vec<(RegionId, u32, EdgeKind)>> = vec![Vec::new(); count];
        for e in &edges {
            adjacency[e.a as usize].push((e.b, e.length_km, e.kind));
            adjacency[e.b as usize].push((e.a, e.length_km, e.kind));
        }
        for list in adjacency.iter_mut() {
            list.sort_by_key(|entry| entry.0);
        }

        let total_value: u64 = regions.iter().map(|r| r.value as u64).sum();
        let map = WorldMap {
            regions,
            edges,
            raster: Raster {
                width,
                height,
                cells_per_degree: cpd,
                lat_top,
                lon_left,
                cells,
            },
            adjacency,
            code_index,
            total_value,
        };

        let dist = map.hop_distances(&[0]);
        if let Some(pos) = dist.iter().position(|&d| d == u32::MAX) {
            return Err(MapError::Invalid(format!(
                "the region graph is not connected ('{}' is unreachable)",
                map.regions[pos].code
            )));
        }
        Ok(map)
    }

    /// The map embedded in the binary (`assets/world_regions.json` and
    /// `assets/world_bitmap.bin`), parsed once and cached.
    ///
    /// Panics only if the shipped asset is corrupt.
    pub fn builtin() -> Arc<WorldMap> {
        let map = BUILTIN_MAP.get_or_init(|| {
            let json: &str = include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../assets/world_regions.json"
            ));
            let bin: &[u8] = include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../assets/world_bitmap.bin"
            ));
            match WorldMap::load(json, bin) {
                Ok(m) => Arc::new(m),
                Err(e) => panic!("the built-in world map asset is corrupt: {}", e),
            }
        });
        Arc::clone(map)
    }

    /// Neighbors of `r` as `(region, edge length in km, kind)`, sorted by region id.
    /// Returns an empty slice for an unknown region.
    pub fn neighbors(&self, r: RegionId) -> &[(RegionId, u32, EdgeKind)] {
        match self.adjacency.get(r as usize) {
            Some(list) => list.as_slice(),
            None => &[],
        }
    }

    /// The edge between `a` and `b`, in either order.
    pub fn edge_between(&self, a: RegionId, b: RegionId) -> Option<&Edge> {
        let key = if a < b { (a, b) } else { (b, a) };
        match self.edges.binary_search_by(|e| (e.a, e.b).cmp(&key)) {
            Ok(i) => self.edges.get(i),
            Err(_) => None,
        }
    }

    /// Whether `a` and `b` share an edge.
    pub fn are_adjacent(&self, a: RegionId, b: RegionId) -> bool {
        self.edge_between(a, b).is_some()
    }

    /// Static info for region `r`. Panics if `r` is out of range.
    pub fn region(&self, r: RegionId) -> &RegionInfo {
        &self.regions[r as usize]
    }

    /// Static info for region `r`, or `None` if it is out of range.
    pub fn get_region(&self, r: RegionId) -> Option<&RegionInfo> {
        self.regions.get(r as usize)
    }

    /// Finds a region by its exact code, ignoring ASCII case.
    pub fn find_code(&self, code: &str) -> Option<RegionId> {
        let key = code.trim().to_ascii_lowercase();
        self.code_index.get(&key).copied()
    }

    /// Sum of all region values.
    pub fn total_value(&self) -> u64 {
        self.total_value
    }

    /// Number of regions.
    pub fn len(&self) -> usize {
        self.regions.len()
    }

    /// Whether the map has no regions (only true for the placeholder default map).
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// Graph hop distance from the nearest of `sources` to every region
    /// (`u32::MAX` for unreachable regions). Invalid sources are ignored.
    pub fn hop_distances(&self, sources: &[RegionId]) -> Vec<u32> {
        let n = self.regions.len();
        let mut dist: Vec<u32> = vec![u32::MAX; n];
        let mut queue: VecDeque<RegionId> = VecDeque::new();
        for &s in sources {
            if (s as usize) < n && dist[s as usize] != 0 {
                dist[s as usize] = 0;
                queue.push_back(s);
            }
        }
        while let Some(cur) = queue.pop_front() {
            let d = dist[cur as usize];
            for &(next, _, _) in self.neighbors(cur) {
                let ni = next as usize;
                if ni < n && dist[ni] == u32::MAX {
                    dist[ni] = d + 1;
                    queue.push_back(next);
                }
            }
        }
        dist
    }

    /// Checks the mapgen preset rules from `ARCHITECTURE.md` 2.3 that `load` does
    /// not enforce: 150 to 220 regions and no region above 4% of total value.
    pub fn check_preset_rules(&self) -> Result<(), MapError> {
        let n = self.regions.len();
        if !(150..=220).contains(&n) {
            return Err(MapError::Invalid(format!(
                "expected 150 to 220 regions, found {}",
                n
            )));
        }
        for r in &self.regions {
            if (r.value as u64) * 100 > self.total_value * 4 {
                return Err(MapError::Invalid(format!(
                    "region '{}' holds more than 4% of world value",
                    r.code
                )));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Private serde model of world_regions.json. Numbers are read as generic JSON
// numbers so that a float such as `468.0` written by the generator still loads.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RegionsFile {
    version: serde_json::Number,
    raster: RasterJson,
    regions: Vec<RegionJson>,
    edges: Vec<EdgeJson>,
}

#[derive(Deserialize)]
struct RasterJson {
    width: serde_json::Number,
    height: serde_json::Number,
    cells_per_degree: serde_json::Number,
    lat_top: serde_json::Number,
    lon_left: serde_json::Number,
}

#[derive(Deserialize)]
struct RegionJson {
    id: serde_json::Number,
    code: String,
    name: String,
    #[serde(default)]
    continent: String,
    value: serde_json::Number,
    #[serde(default)]
    area_km2: Option<serde_json::Number>,
    #[serde(default)]
    population: Option<serde_json::Number>,
    label: Vec<serde_json::Number>,
}

#[derive(Deserialize)]
struct EdgeJson {
    a: serde_json::Number,
    b: serde_json::Number,
    kind: EdgeKind,
    length_km: serde_json::Number,
}

/// Converts a JSON number to i64 (integers exactly, floats rounded).
fn num_to_i64(n: &serde_json::Number, what: &str) -> Result<i64, MapError> {
    if let Some(v) = n.as_i64() {
        return Ok(v);
    }
    if let Some(v) = n.as_u64() {
        return Ok(if v > i64::MAX as u64 { i64::MAX } else { v as i64 });
    }
    match n.as_f64() {
        Some(f) if f.is_finite() => {
            let r = f.round();
            if r >= i64::MAX as f64 {
                Ok(i64::MAX)
            } else if r <= i64::MIN as f64 {
                Ok(i64::MIN)
            } else {
                Ok(r as i64)
            }
        }
        _ => Err(MapError::Invalid(format!("{} is not a valid number", what))),
    }
}

/// Converts a JSON number to i64 and checks it lies in `lo..=hi`.
fn num_in_range(n: &serde_json::Number, what: &str, lo: i64, hi: i64) -> Result<i64, MapError> {
    let v = num_to_i64(n, what)?;
    if v < lo || v > hi {
        return Err(MapError::Invalid(format!(
            "{} = {} is out of range {}..={}",
            what, v, lo, hi
        )));
    }
    Ok(v)
}

/// Bounds-checked little-endian reader over a byte slice.
struct ByteReader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> ByteReader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], MapError> {
        let all: &'a [u8] = self.bytes;
        let end = match self.pos.checked_add(n) {
            Some(e) => e,
            None => return Err(MapError::Bitmap("offset overflow".to_string())),
        };
        if end > all.len() {
            return Err(MapError::Bitmap(format!(
                "unexpected end of data at byte {}",
                self.pos
            )));
        }
        let slice = &all[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn read_u16(&mut self) -> Result<u16, MapError> {
        let s = self.take(2)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }
}

/// Decodes the RLE bitmap into `width * height` row-major cells.
fn decode_bitmap(
    bytes: &[u8],
    width: u16,
    height: u16,
    region_count: usize,
) -> Result<Vec<u16>, MapError> {
    let mut rd = ByteReader { bytes, pos: 0 };
    let magic = rd.take(4)?;
    if magic != &b"TBMP"[..] {
        return Err(MapError::Bitmap("bad magic (expected TBMP)".to_string()));
    }
    let version = rd.read_u16()?;
    if version != 1 {
        return Err(MapError::Bitmap(format!("unsupported version {}", version)));
    }
    let bw = rd.read_u16()?;
    let bh = rd.read_u16()?;
    if bw != width || bh != height {
        return Err(MapError::Bitmap(format!(
            "size {}x{} does not match the JSON raster {}x{}",
            bw, bh, width, height
        )));
    }
    let w = width as usize;
    let mut cells: Vec<u16> = Vec::with_capacity(w * height as usize);
    for y in 0..height {
        let runs = rd.read_u16()?;
        let mut row_len: usize = 0;
        for _ in 0..runs {
            let region = rd.read_u16()?;
            let len = rd.read_u16()? as usize;
            if region != OCEAN && region as usize >= region_count {
                return Err(MapError::Bitmap(format!(
                    "row {} references unknown region {}",
                    y, region
                )));
            }
            row_len += len;
            if row_len > w {
                return Err(MapError::Bitmap(format!("row {} is longer than {}", y, w)));
            }
            cells.extend(std::iter::repeat(region).take(len));
        }
        if row_len != w {
            return Err(MapError::Bitmap(format!(
                "row {} has {} cells, expected {}",
                y, row_len, w
            )));
        }
    }
    Ok(cells)
}
