# mapgen: Natural Earth to Territorial CLI map assets

`mapgen.py` is the build-time map pipeline (PRD milestone M0). It turns the
Natural Earth 1:50m admin-0 countries GeoJSON into the two files that the game
embeds:

- `assets/world_regions.json` holds the regions, the adjacency edges and the raster metadata.
- `assets/world_bitmap.bin` is the RLE region-id raster (`TBMP` v1).

Both formats are specified in `docs/ARCHITECTURE.md` section 2. The generated
files are committed, so normal builds need neither Python nor network access.

The tool uses only the Python standard library (Python 3.10 or newer). It is
deterministic: the same input and curation always produce byte-identical output.

## Running

```sh
# with a GeoJSON you already have
python3 -I mapgen/mapgen.py --input path/to/ne_50m_admin_0_countries.geojson --out assets

# or download it into mapgen/cache/ (gitignored) and use that
python3 -I mapgen/mapgen.py --download --out assets
```

Useful flags:

| Flag | Effect |
| --- | --- |
| `--preview` | Prints a 180x70 ASCII world map, one character per 8x8 block (the first letter of the region code). |
| `--preview-box LON0 LON1 LAT0 LAT1` | Prints a detailed view of a lon/lat box, one symbol per region plus a legend. `#` marks label cells. Repeatable. Use it to check split boxes. |
| `--dry-run` | Runs and validates everything but writes nothing. |
| `--curation FILE` | Uses a different curation file. The default is `mapgen/curation.json`. |

A full run takes about 1 to 2 seconds. Run Python with `-I`, because the
GeoJSON is downloaded data. The script exits non-zero if the curation is
inconsistent or the output fails validation, and it writes nothing in that case.

## Pipeline

1. **Units.** Each GeoJSON feature is keyed by `ADM0_A3`. Antarctica and the
   `drop` list are removed. Features in `merge` (micro-states, crown
   dependencies, Hong Kong and Macao, N. Cyprus, Somaliland, Siachen) are
   folded into their target, which also gets their population.
2. **Rasterize.** The grid is equirectangular at 4 cells per degree. It covers
   lon -180..180 (1440 columns) and lat 84..-56 (560 rows). Each polygon is
   filled with a scanline even-odd rule at cell centers, so holes and
   MultiPolygons work. A cell that is already owned is never overwritten.
3. **Partition.** Countries listed in `partitions` have their cells assigned
   by ordered lon/lat box rules. The first matching rule wins, and the last
   rule must be the catch-all `[-180, 180, -90, 90]`. A rule either names a
   region (`code`, `name`, optional `continent`, optional `pop_weight`) or
   drops the cell (`"drop": true`). The same mechanism clips far-off overseas
   parts: France keeps the mainland and Corsica, splits out French Guiana as
   `gf`, and drops the rest. Chile, Ecuador, New Zealand, Norway, the
   Netherlands and South Africa are clipped the same way.
4. **Population.** A country's `POP_EST` (plus merged populations) is split
   across its sub-regions by `pop_weight` when every sub-region has one.
   Otherwise it is split by area.
5. **Small regions.** A `keep_small` country that rasterizes to 0 cells gets
   the cell under its label point. Any other unsplit region with fewer than
   `min_cells` cells is merged into the neighbor it touches most. If it
   touches nothing, it is dropped.
6. **Sort.** Regions are sorted by `code` and given ids `0..n-1`.
7. **Metrics.** `area_km2` is the sum of exact cell areas on a sphere (R =
   6371 km). Unsplit countries use Natural Earth's `LABEL_X`/`LABEL_Y` as the
   label when that cell is inside the region. Otherwise the label is the cell
   of the largest connected piece that is nearest its centroid. Only cells at
   least 2 cells from the piece's edge are considered, so labels avoid coasts.
8. **Value.** The value is
   `clamp(round(area_coef*sqrt(area_km2/1000) + pop_coef*sqrt(population/1e6)), 1, 30)`,
   with `value_overrides` applied last. The overrides lift the major
   economies, which area and population alone undervalue.
9. **Edges.**
   - Land edges come from raster contact between two regions. The check uses
     the 4-neighborhood plus diagonals, with no wrap at the antimeridian.
     `remove_edges` can delete a spurious contact.
   - Curated `sea_lanes` are then added. A lane that duplicates a land edge
     is skipped and reported.
   - Any region still without edges gets lanes to its 2 nearest regions by
     label distance.
   - While the graph is disconnected, each smaller component is joined to the
     largest one by its closest label pair.
   - `length_km` is the great-circle distance between the two labels, with a
     minimum of 50.
10. **Validate and write.** The script checks the following:
    - The region count is 150..=220.
    - Ids are contiguous and regions are sorted by code.
    - Codes are unique and match `[a-z0-9-]`.
    - Each continent is one of the six allowed values.
    - Every region has at least 1 cell and its label is inside it.
    - Values are 1..=30 and no region holds more than 4% of the total.
    - Edges have `a < b`, are unique, and have `length_km >= 50`.
    - The graph is connected.
    - Every bitmap row's run lengths sum to the width, and the per-region cell
      counts in the bitmap match the JSON.

## Curation (`curation.json`)

| Key | Meaning |
| --- | --- |
| `min_cells` | Threshold for automatic micro-region merging. |
| `drop` | ADM0_A3 codes to remove (remote dependencies and tiny island states that would only add noise). |
| `merge` | `source ADM0_A3 -> target ADM0_A3`. |
| `keep_small` | ADM0_A3 codes that are exempt from automatic merging and get stamped if they rasterize to nothing. |
| `name_overrides` | ADM0_A3 -> display name. Names are otherwise Natural Earth `NAME`, folded to ASCII. |
| `code_overrides` | ADM0_A3 -> code. Codes are otherwise `ISO_A2_EH` lowercased, or `ADM0_A3` lowercased when that is `-99`. |
| `continent_overrides` | ADM0_A3 -> continent. |
| `partitions` | ADM0_A3 -> ordered box rules (see Pipeline step 3). |
| `value` | The formula constants. |
| `value_overrides` | Region code -> final value. |
| `remove_edges` | `[code, code]` land contacts to delete. |
| `sea_lanes` | `[code, code]` sea edges to add. |

Keys that start with `_` are comments and are ignored.

Boxes are `[lon_min, lon_max, lat_min, lat_max]`. They are tested against cell
centers, with the minimum inclusive and the maximum exclusive. When you edit a
box, check the result with `--preview-box` over the area.

## Current output

There are 214 regions, 482 edges (410 land, 72 sea) and a total value of 1572.
The median value is 6 and the maximum is 26 (`cn-e`), which is a 1.65% share.

| Country | Sub-regions |
| --- | --- |
| United States | `us-w` (Pacific West + Hawaii), `us-m`, `us-s`, `us-c`, `us-se`, `us-ne`, `us-ak` |
| Canada | `ca-bc`, `ca-pr`, `ca-on`, `ca-qc`, `ca-at`, `ca-n` |
| Russia | `ru-nw` (with Kaliningrad), `ru-c`, `ru-s`, `ru-u`, `ru-ws`, `ru-sib`, `ru-y`, `ru-am`, `ru-e` |
| China | `cn-xj`, `cn-xz`, `cn-nw`, `cn-n`, `cn-ne`, `cn-e`, `cn-s`, `cn-sw` |
| Brazil | `br-n`, `br-ne`, `br-co`, `br-se`, `br-s` |
| Australia | `au-w`, `au-c`, `au-q`, `au-se` |
| India | `in-n`, `in-ne`, `in-e`, `in-w`, `in-c`, `in-s` |
| Indonesia | `id-w`, `id-k`, `id-e` |
| France | `fr`, plus French Guiana as `gf` |
