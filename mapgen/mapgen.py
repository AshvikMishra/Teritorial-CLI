#!/usr/bin/env python3
"""Territorial CLI map pipeline (PRD milestone M0).

Turns the Natural Earth 1:50m admin-0 countries GeoJSON into the two map
assets described in docs/ARCHITECTURE.md section 2:

    assets/world_regions.json   regions, edges, raster metadata
    assets/world_bitmap.bin     RLE region-id raster ("TBMP" v1)

Standard library only (Python 3.10+). All hand-tuned data lives in
mapgen/curation.json. The pipeline is deterministic: the same input and
curation always produce byte-identical output.

Usage:
    python3 -I mapgen/mapgen.py --input <countries.geojson> --out assets
    python3 -I mapgen/mapgen.py --download --out assets
    python3 -I mapgen/mapgen.py --input ... --out assets --preview
    python3 -I mapgen/mapgen.py --input ... --out assets --preview-box -130 -60 22 52
"""

import argparse
import json
import math
import os
import re
import struct
import sys
import unicodedata
import urllib.request
from collections import deque

# ---------------------------------------------------------------------------
# Raster geometry (ARCHITECTURE.md 2.1)
# ---------------------------------------------------------------------------

CPD = 4                     # cells per degree
LON_LEFT = -180
LAT_TOP = 84
LAT_BOTTOM = -56
WIDTH = 360 * CPD           # 1440
HEIGHT = (LAT_TOP - LAT_BOTTOM) * CPD   # 560
NCELLS = WIDTH * HEIGHT
OCEAN = 0xFFFF
NONE = -1

EARTH_RADIUS_KM = 6371.0088
MIN_EDGE_KM = 50
MIN_REGIONS = 150
MAX_REGIONS = 220
MAX_VALUE = 30
MAX_VALUE_SHARE = 0.04

CONTINENTS = ("Africa", "Asia", "Europe", "North America", "South America", "Oceania")

DOWNLOAD_URL = ("https://raw.githubusercontent.com/nvkelso/natural-earth-vector/"
                "master/geojson/ne_50m_admin_0_countries.geojson")
DOWNLOAD_NAME = "ne_50m_admin_0_countries.geojson"

CODE_RE = re.compile(r"^[a-z0-9]+(-[a-z0-9]+)*$")

# Precomputed cell-center coordinates.
COL_LON = [LON_LEFT + (x + 0.5) / CPD for x in range(WIDTH)]
ROW_LAT = [LAT_TOP - (y + 0.5) / CPD for y in range(HEIGHT)]


def _row_area_km2(y):
    top = math.radians(LAT_TOP - y / CPD)
    bot = math.radians(LAT_TOP - (y + 1) / CPD)
    dlon = math.radians(1.0 / CPD)
    return EARTH_RADIUS_KM * EARTH_RADIUS_KM * dlon * (math.sin(top) - math.sin(bot))


ROW_AREA = [_row_area_km2(y) for y in range(HEIGHT)]


class MapgenError(Exception):
    """A curation or data problem that makes the output invalid."""


def fail(msg):
    raise MapgenError(msg)


def info(msg=""):
    print(msg)


# ---------------------------------------------------------------------------
# Input
# ---------------------------------------------------------------------------

def download(cache_dir):
    os.makedirs(cache_dir, exist_ok=True)
    path = os.path.join(cache_dir, DOWNLOAD_NAME)
    if os.path.exists(path) and os.path.getsize(path) > 0:
        info("using cached download: %s" % path)
        return path
    info("downloading %s" % DOWNLOAD_URL)
    with urllib.request.urlopen(DOWNLOAD_URL, timeout=120) as resp:
        data = resp.read()
    tmp = path + ".part"
    with open(tmp, "wb") as fh:
        fh.write(data)
    os.replace(tmp, path)
    info("saved %d bytes to %s" % (len(data), path))
    return path


def load_json(path):
    with open(path, "r", encoding="utf-8") as fh:
        return json.load(fh)


def ascii_fold(text):
    norm = unicodedata.normalize("NFKD", text)
    out = "".join(ch for ch in norm if not unicodedata.combining(ch))
    out = out.encode("ascii", "ignore").decode("ascii")
    return " ".join(out.split())


def polygons_of(geometry):
    """Return a list of polygons; each polygon is a list of rings of [lon, lat]."""
    if geometry is None:
        return []
    gtype = geometry.get("type")
    coords = geometry.get("coordinates")
    if gtype == "Polygon":
        return [coords]
    if gtype == "MultiPolygon":
        return list(coords)
    return []


# ---------------------------------------------------------------------------
# Rasterization: scanline even-odd fill at cell centers
# ---------------------------------------------------------------------------

def rasterize_polygon(rings, grid, value):
    """Fill every cell whose center lies inside the polygon (even-odd over all
    rings, so holes work). Cells already owned by someone else are kept."""
    rows = {}
    for ring in rings:
        n = len(ring)
        if n < 3:
            continue
        for i in range(n):
            p = ring[i]
            q = ring[(i + 1) % n]
            x1 = float(p[0])
            y1 = float(p[1])
            x2 = float(q[0])
            y2 = float(q[1])
            if y1 == y2:
                continue
            if y1 < y2:
                ymin, ymax = y1, y2
            else:
                ymin, ymax = y2, y1
            # Rows whose center latitude c satisfies ymin <= c < ymax.
            r0 = math.floor((LAT_TOP - ymax) * CPD - 0.5) + 1
            r1 = math.floor((LAT_TOP - ymin) * CPD - 0.5)
            if r0 < 0:
                r0 = 0
            if r1 > HEIGHT - 1:
                r1 = HEIGHT - 1
            if r0 > r1:
                continue
            slope = (x2 - x1) / (y2 - y1)
            for r in range(r0, r1 + 1):
                c = ROW_LAT[r]
                lst = rows.get(r)
                if lst is None:
                    lst = []
                    rows[r] = lst
                lst.append(x1 + (c - y1) * slope)
    filled = 0
    for r, xs in rows.items():
        xs.sort()
        base = r * WIDTH
        for k in range(0, len(xs) - 1, 2):
            xa = math.ceil((xs[k] - LON_LEFT) * CPD - 0.5)
            xb = math.ceil((xs[k + 1] - LON_LEFT) * CPD - 0.5)
            if xa < 0:
                xa = 0
            if xb > WIDTH:
                xb = WIDTH
            for x in range(xa, xb):
                idx = base + x
                if grid[idx] == NONE:
                    grid[idx] = value
                    filled += 1
    return filled


def cell_of_lonlat(lon, lat):
    x = int(math.floor((lon - LON_LEFT) * CPD))
    y = int(math.floor((LAT_TOP - lat) * CPD))
    if x < 0:
        x = 0
    if x >= WIDTH:
        x = WIDTH - 1
    if y < 0 or y >= HEIGHT:
        return None
    return y * WIDTH + x


def cell_lonlat(idx):
    return COL_LON[idx % WIDTH], ROW_LAT[idx // WIDTH]


def great_circle_km(lon1, lat1, lon2, lat2):
    p1 = math.radians(lat1)
    p2 = math.radians(lat2)
    dp = p2 - p1
    dl = math.radians(lon2 - lon1)
    h = math.sin(dp / 2) ** 2 + math.cos(p1) * math.cos(p2) * math.sin(dl / 2) ** 2
    if h > 1.0:
        h = 1.0
    return 2.0 * EARTH_RADIUS_KM * math.asin(math.sqrt(h))


def round_half_up(v):
    return int(math.floor(v + 0.5))


# ---------------------------------------------------------------------------
# Curation helpers
# ---------------------------------------------------------------------------

def cur_dict(cur, key):
    """A curation object with documentation keys (leading underscore) removed."""
    val = cur.get(key, {})
    if not isinstance(val, dict):
        fail("curation %s must be an object" % key)
    return dict((k, v) for k, v in val.items() if not k.startswith("_"))


def box_match(boxes, lon, lat):
    for b in boxes:
        if b[0] <= lon < b[1] and b[2] <= lat < b[3]:
            return True
    return False


def check_boxes(boxes, where):
    if not isinstance(boxes, list) or not boxes:
        fail("%s: 'boxes' must be a non-empty list" % where)
    for b in boxes:
        if not isinstance(b, list) or len(b) != 4:
            fail("%s: each box is [lon_min, lon_max, lat_min, lat_max], got %r" % (where, b))
        if not (b[0] < b[1] and b[2] < b[3]):
            fail("%s: empty box %r" % (where, b))


class Unit(object):
    """One Natural Earth feature after drops and merges (keyed by ADM0_A3)."""

    def __init__(self, adm0, props, feature):
        self.adm0 = adm0
        self.props = props
        self.features = [feature]
        self.population = float(props.get("POP_EST") or 0)
        self.merged_from = []


class Region(object):
    def __init__(self, code, name, continent, unit, is_main, pop_weight):
        self.code = code
        self.name = name
        self.continent = continent
        self.unit = unit
        self.is_main = is_main          # True when this region is the unit's own (unsplit or main part)
        self.pop_weight = pop_weight
        self.split = False
        self.cells = []
        self.population = 0.0
        self.area_km2 = 0.0
        self.label = None
        self.value = 0
        self.alive = True


# ---------------------------------------------------------------------------
# Pipeline
# ---------------------------------------------------------------------------

class Pipeline(object):
    def __init__(self, geo, cur):
        self.geo = geo
        self.cur = cur
        self.units = []
        self.unit_by_adm0 = {}
        self.regions = []
        self.grid = None
        self.report_merges = []
        self.report_drops = []
        self.report_stamps = []
        self.report_auto_lanes = []
        self.report_skipped_lanes = []
        self.edges = {}
        self.unit_grid = None
        self.unit_rules = []

    # ---- step 1: units -----------------------------------------------------
    def build_units(self):
        cur = self.cur
        drops = set(cur.get("drop", []))
        merges = cur_dict(cur, "merge")
        by_adm0 = {}
        for f in self.geo.get("features", []):
            props = f.get("properties") or {}
            adm0 = props.get("ADM0_A3")
            if not adm0:
                fail("feature without ADM0_A3: %r" % props.get("NAME"))
            if adm0 in by_adm0:
                fail("duplicate ADM0_A3 %s" % adm0)
            by_adm0[adm0] = f
        for key in sorted(drops):
            if key not in by_adm0:
                fail("curation drop: unknown ADM0_A3 %s" % key)
        for src in sorted(merges):
            if src not in by_adm0:
                fail("curation merge: unknown source ADM0_A3 %s" % src)
            dst = merges[src]
            if dst not in by_adm0 or dst in drops or dst in merges:
                fail("curation merge: bad target %s for %s" % (dst, src))
        for adm0 in sorted(by_adm0):
            f = by_adm0[adm0]
            props = f["properties"]
            if adm0 in drops:
                self.report_drops.append("%s (%s): curated drop" % (ascii_fold(props.get("NAME", "")), adm0))
                continue
            if props.get("CONTINENT") == "Antarctica":
                self.report_drops.append("%s (%s): Antarctica" % (props.get("NAME"), adm0))
                continue
            if adm0 in merges:
                continue
            u = Unit(adm0, props, f)
            self.unit_by_adm0[adm0] = u
            self.units.append(u)
        for src in sorted(merges):
            if src in drops:
                continue
            dst = self.unit_by_adm0[merges[src]]
            f = by_adm0[src]
            dst.features.append(f)
            dst.population += float(f["properties"].get("POP_EST") or 0)
            dst.merged_from.append(src)
            self.report_merges.append("%s (%s) -> %s (curated)" % (
                ascii_fold(f["properties"].get("NAME", "")), src, dst.adm0))
        for key in ("partitions", "keep_small", "name_overrides", "code_overrides"):
            val = cur_dict(cur, key) if key != "keep_small" else cur.get(key, [])
            for adm0 in val:
                if adm0 not in self.unit_by_adm0:
                    fail("curation %s: %s is not a surviving unit" % (key, adm0))

    # ---- step 2: rasterize -------------------------------------------------
    def rasterize(self):
        grid = [NONE] * NCELLS
        for ui, u in enumerate(self.units):
            for f in u.features:
                for poly in polygons_of(f.get("geometry")):
                    rasterize_polygon(poly, grid, ui)
        self.unit_grid = grid

    # ---- step 3: regions (partition) ---------------------------------------
    def unit_code(self, u):
        overrides = cur_dict(self.cur, "code_overrides")
        if u.adm0 in overrides:
            return overrides[u.adm0]
        iso = str(u.props.get("ISO_A2_EH") or "").strip().lower()
        if re.match(r"^[a-z]{2}$", iso):
            return iso
        return u.adm0.lower()

    def unit_name(self, u):
        overrides = cur_dict(self.cur, "name_overrides")
        if u.adm0 in overrides:
            return overrides[u.adm0]
        return ascii_fold(str(u.props.get("NAME") or u.adm0))

    def unit_continent(self, u):
        overrides = cur_dict(self.cur, "continent_overrides")
        if u.adm0 in overrides:
            return overrides[u.adm0]
        return str(u.props.get("CONTINENT") or "")

    def build_regions(self):
        partitions = cur_dict(self.cur, "partitions")
        self.unit_rules = []
        for u in self.units:
            code = self.unit_code(u)
            name = self.unit_name(u)
            cont = self.unit_continent(u)
            spec = partitions.get(u.adm0)
            if spec is None:
                r = Region(code, name, cont, u, True, None)
                self.regions.append(r)
                self.unit_rules.append(len(self.regions) - 1)
                continue
            rules = []
            created = {}
            where = "partition %s" % u.adm0
            if not isinstance(spec, list) or not spec:
                fail("%s: must be a non-empty list of rules" % where)
            for k, rule in enumerate(spec):
                rwhere = "%s rule %d" % (where, k)
                if not isinstance(rule, dict):
                    fail("%s: must be an object" % rwhere)
                boxes = rule.get("boxes")
                check_boxes(boxes, rwhere)
                if rule.get("drop"):
                    rules.append((boxes, NONE))
                    continue
                rcode = rule.get("code")
                if not rcode:
                    fail("%s: needs 'code' or 'drop'" % rwhere)
                if rcode in created:
                    rules.append((boxes, created[rcode]))
                    continue
                is_main = (rcode == code)
                rname = rule.get("name", name if is_main else None)
                if not rname:
                    fail("%s: sub-region %s needs a name" % (rwhere, rcode))
                rcont = rule.get("continent", cont)
                reg = Region(rcode, rname, rcont, u, is_main, rule.get("pop_weight"))
                self.regions.append(reg)
                created[rcode] = len(self.regions) - 1
                rules.append((boxes, created[rcode]))
            last = spec[-1]["boxes"]
            if not any(b[0] <= -180 and b[1] >= 180 and b[2] <= LAT_BOTTOM and b[3] >= LAT_TOP for b in last):
                fail("%s: the last rule must be a catch-all box [-180, 180, -90, 90]" % where)
            if len(created) > 1:
                for ri in created.values():
                    self.regions[ri].split = True
            self.unit_rules.append(rules)

        grid = [NONE] * NCELLS
        ugrid = self.unit_grid
        for idx in range(NCELLS):
            ui = ugrid[idx]
            if ui == NONE:
                continue
            rules = self.unit_rules[ui]
            if isinstance(rules, int):
                ri = rules
            else:
                lon = COL_LON[idx % WIDTH]
                lat = ROW_LAT[idx // WIDTH]
                ri = NONE
                for boxes, target in rules:
                    if box_match(boxes, lon, lat):
                        ri = target
                        break
            if ri != NONE:
                grid[idx] = ri
                self.regions[ri].cells.append(idx)
        self.grid = grid

        # A partition must not produce empty sub-regions.
        for r in self.regions:
            if r.split and not r.cells:
                fail("partition %s: sub-region %s got no cells; fix its boxes" % (r.unit.adm0, r.code))

    # ---- step 4: populations -----------------------------------------------
    def assign_populations(self):
        by_unit = {}
        for ri, r in enumerate(self.regions):
            by_unit.setdefault(r.unit.adm0, []).append(ri)
        for adm0 in sorted(by_unit):
            ris = by_unit[adm0]
            u = self.unit_by_adm0[adm0]
            if len(ris) == 1:
                self.regions[ris[0]].population = u.population
                continue
            regs = [self.regions[i] for i in ris]
            if all(r.pop_weight is not None for r in regs):
                weights = [float(r.pop_weight) for r in regs]
            else:
                weights = [self.area_of(r.cells) for r in regs]
            total = sum(weights)
            for r, w in zip(regs, weights):
                r.population = u.population * (w / total if total > 0 else 1.0 / len(regs))

    @staticmethod
    def area_of(cells):
        a = 0.0
        for idx in cells:
            a += ROW_AREA[idx // WIDTH]
        return a

    # ---- step 5: stamping and micro-state handling -------------------------
    def stamp_and_prune(self):
        keep_small = set(self.cur.get("keep_small", []))
        min_cells = int(self.cur.get("min_cells", 3))
        grid = self.grid
        # Kept small countries that rasterized to nothing get their label cell.
        for ri, r in enumerate(self.regions):
            if r.cells or r.split:
                continue
            if r.unit.adm0 not in keep_small:
                continue
            lx = r.unit.props.get("LABEL_X")
            ly = r.unit.props.get("LABEL_Y")
            if lx is None or ly is None:
                fail("%s: no label point to stamp" % r.code)
            idx = cell_of_lonlat(float(lx), float(ly))
            if idx is None:
                fail("%s: label point outside raster" % r.code)
            prev = grid[idx]
            if prev != NONE:
                other = self.regions[prev]
                if len(other.cells) <= 1:
                    fail("%s: stamping would empty %s" % (r.code, other.code))
                other.cells.remove(idx)
            grid[idx] = ri
            r.cells.append(idx)
            self.report_stamps.append("%s stamped at cell (%d, %d)%s" % (
                r.code, idx % WIDTH, idx // WIDTH,
                (" taking it from %s" % self.regions[prev].code) if prev != NONE else ""))

        # Auto merge / drop of micro regions.
        order = sorted(range(len(self.regions)),
                       key=lambda i: (len(self.regions[i].cells), self.regions[i].code))
        for ri in order:
            r = self.regions[ri]
            if r.split or r.unit.adm0 in keep_small:
                continue
            if len(r.cells) >= min_cells:
                continue
            if not r.cells:
                r.alive = False
                self.report_drops.append("%s (%s): rasterized to 0 cells" % (r.name, r.code))
                continue
            counts = {}
            for idx in r.cells:
                for nidx in neighbors8(idx):
                    o = grid[nidx]
                    if o != NONE and o != ri:
                        counts[o] = counts.get(o, 0) + 1
            if counts:
                target = sorted(counts.items(), key=lambda kv: (-kv[1], self.regions[kv[0]].code))[0][0]
                t = self.regions[target]
                for idx in r.cells:
                    grid[idx] = target
                t.cells.extend(r.cells)
                t.population += r.population
                self.report_merges.append("%s (%s, %d cells) -> %s (auto, surrounded)" % (
                    r.name, r.code, len(r.cells), t.code))
            else:
                for idx in r.cells:
                    grid[idx] = NONE
                self.report_drops.append("%s (%s, %d cells): auto, isolated micro island" % (
                    r.name, r.code, len(r.cells)))
            r.cells = []
            r.alive = False

    # ---- step 6: compact and sort -----------------------------------------
    def compact(self):
        alive = [r for r in self.regions if r.alive and r.cells]
        alive.sort(key=lambda r: r.code)
        old_index = {}
        for ri, r in enumerate(self.regions):
            old_index[id(r)] = ri
        remap = {}
        for new_id, r in enumerate(alive):
            remap[old_index[id(r)]] = new_id
        grid = self.grid
        for idx in range(NCELLS):
            v = grid[idx]
            if v != NONE:
                grid[idx] = remap[v]
        for r in alive:
            r.cells.sort()
        self.regions = alive

    # ---- step 7: metrics ---------------------------------------------------
    def compute_metrics(self):
        for ri, r in enumerate(self.regions):
            r.area_km2 = self.area_of(r.cells)
            r.label = self.pick_label(ri, r)

    def pick_label(self, ri, r):
        grid = self.grid
        if r.is_main and not r.split:
            lx = r.unit.props.get("LABEL_X")
            ly = r.unit.props.get("LABEL_Y")
            if lx is not None and ly is not None:
                idx = cell_of_lonlat(float(lx), float(ly))
                if idx is not None and grid[idx] == ri:
                    return idx
        cellset = set(r.cells)
        # Largest 4-connected component (wrapping horizontally).
        seen = set()
        best = None
        for start in r.cells:
            if start in seen:
                continue
            comp = []
            dq = deque([start])
            seen.add(start)
            while dq:
                c = dq.popleft()
                comp.append(c)
                for n in neighbors4_wrap(c):
                    if n in cellset and n not in seen:
                        seen.add(n)
                        dq.append(n)
            if best is None or len(comp) > len(best):
                best = comp
        comp = best
        compset = set(comp)
        # Distance (in cells) from the component edge, so labels avoid coasts.
        dist = {}
        dq = deque()
        for c in comp:
            edge = False
            for n in neighbors4_wrap(c):
                if n not in compset:
                    edge = True
                    break
            if edge:
                dist[c] = 0
                dq.append(c)
        while dq:
            c = dq.popleft()
            for n in neighbors4_wrap(c):
                if n in compset and n not in dist:
                    dist[n] = dist[c] + 1
                    dq.append(n)
        maxd = max(dist.values()) if dist else 0
        need = min(maxd, 2)
        # Circular mean in x, plain mean in y.
        sx = 0.0
        cx = 0.0
        sy = 0.0
        for c in comp:
            ang = 2.0 * math.pi * ((c % WIDTH) + 0.5) / WIDTH
            sx += math.sin(ang)
            cx += math.cos(ang)
            sy += c // WIDTH
        ang = math.atan2(sx, cx)
        if ang < 0:
            ang += 2.0 * math.pi
        mx = ang * WIDTH / (2.0 * math.pi) - 0.5
        my = sy / len(comp)
        best_c = None
        best_d = None
        for c in sorted(comp):
            if dist.get(c, 0) < need:
                continue
            dx = abs((c % WIDTH) - mx)
            if dx > WIDTH / 2:
                dx = WIDTH - dx
            dy = (c // WIDTH) - my
            d = dx * dx + dy * dy
            if best_d is None or d < best_d:
                best_d = d
                best_c = c
        return best_c

    # ---- step 8: values ----------------------------------------------------
    def compute_values(self):
        vc = self.cur.get("value", {})
        a = float(vc.get("area_coef", 0.15))
        b = float(vc.get("pop_coef", 1.0))
        area_unit = float(vc.get("area_unit_km2", 1000.0))
        pop_unit = float(vc.get("pop_unit", 1000000.0))
        overrides = cur_dict(self.cur, "value_overrides")
        codes = set(r.code for r in self.regions)
        for k in overrides:
            if k not in codes:
                fail("value_overrides: unknown code %s" % k)
        for r in self.regions:
            raw = a * math.sqrt(r.area_km2 / area_unit) + b * math.sqrt(r.population / pop_unit)
            v = round_half_up(raw)
            if r.code in overrides:
                v = int(overrides[r.code])
            r.value = max(1, min(MAX_VALUE, v))

    # ---- step 9: adjacency -------------------------------------------------
    def compute_land_edges(self):
        grid = self.grid
        contacts = {}
        for y in range(HEIGHT):
            base = y * WIDTH
            for x in range(WIDTH):
                a = grid[base + x]
                if a == NONE:
                    continue
                nbrs = []
                if x + 1 < WIDTH:
                    nbrs.append(grid[base + x + 1])
                if y + 1 < HEIGHT:
                    nb = base + WIDTH
                    nbrs.append(grid[nb + x])
                    if x + 1 < WIDTH:
                        nbrs.append(grid[nb + x + 1])
                    if x - 1 >= 0:
                        nbrs.append(grid[nb + x - 1])
                for b in nbrs:
                    if b == NONE or b == a:
                        continue
                    key = (a, b) if a < b else (b, a)
                    contacts[key] = contacts.get(key, 0) + 1
        removed = set()
        code_ix = self.code_index()
        for pair in self.cur.get("remove_edges", []):
            ka = code_ix.get(pair[0])
            kb = code_ix.get(pair[1])
            if ka is None or kb is None:
                fail("remove_edges: unknown code in %r" % (pair,))
            removed.add((min(ka, kb), max(ka, kb)))
        for key in sorted(contacts):
            if key in removed:
                continue
            self.edges[key] = "land"
        for key in sorted(removed):
            if key not in contacts:
                fail("remove_edges: %s-%s is not a land contact" % (
                    self.regions[key[0]].code, self.regions[key[1]].code))

    def code_index(self):
        return dict((r.code, i) for i, r in enumerate(self.regions))

    def label_ll(self, ri):
        return cell_lonlat(self.regions[ri].label)

    def label_dist(self, a, b):
        la = self.label_ll(a)
        lb = self.label_ll(b)
        return great_circle_km(la[0], la[1], lb[0], lb[1])

    def add_sea(self, a, b, why):
        key = (a, b) if a < b else (b, a)
        if key in self.edges:
            return False
        self.edges[key] = "sea"
        if why:
            self.report_auto_lanes.append("%s-%s (%s, %d km)" % (
                self.regions[key[0]].code, self.regions[key[1]].code, why,
                round_half_up(self.label_dist(key[0], key[1]))))
        return True

    def compute_sea_edges(self):
        code_ix = self.code_index()
        for pair in self.cur.get("sea_lanes", []):
            if not isinstance(pair, list) or len(pair) != 2:
                fail("sea_lanes: bad entry %r" % (pair,))
            ka = code_ix.get(pair[0])
            kb = code_ix.get(pair[1])
            if ka is None or kb is None:
                fail("sea_lanes: unknown code in %r" % (pair,))
            if ka == kb:
                fail("sea_lanes: self loop %r" % (pair,))
            if not self.add_sea(ka, kb, None):
                self.report_skipped_lanes.append("%s-%s" % (pair[0], pair[1]))
        n = len(self.regions)
        # Isolated regions: connect to the 2 nearest by label distance.
        deg = [0] * n
        for (a, b) in self.edges:
            deg[a] += 1
            deg[b] += 1
        for ri in range(n):
            if deg[ri] > 0:
                continue
            cands = sorted((self.label_dist(ri, o), o) for o in range(n) if o != ri)
            for d, o in cands[:2]:
                if self.add_sea(ri, o, "isolated"):
                    deg[ri] += 1
                    deg[o] += 1
        # Connect components to the main one.
        while True:
            comps = self.components()
            if len(comps) <= 1:
                break
            comps.sort(key=lambda c: (-len(c), min(c)))
            main = comps[0]
            for comp in comps[1:]:
                best = None
                for a in comp:
                    for b in main:
                        d = self.label_dist(a, b)
                        if best is None or d < best[0] or (d == best[0] and (a, b) < (best[1], best[2])):
                            best = (d, a, b)
                self.add_sea(best[1], best[2], "connectivity")

    def components(self):
        n = len(self.regions)
        adj = [[] for _ in range(n)]
        for (a, b) in self.edges:
            adj[a].append(b)
            adj[b].append(a)
        seen = [False] * n
        comps = []
        for s in range(n):
            if seen[s]:
                continue
            comp = []
            dq = deque([s])
            seen[s] = True
            while dq:
                c = dq.popleft()
                comp.append(c)
                for o in adj[c]:
                    if not seen[o]:
                        seen[o] = True
                        dq.append(o)
            comps.append(sorted(comp))
        return comps

    # ---- outputs -----------------------------------------------------------
    def region_records(self):
        out = []
        for ri, r in enumerate(self.regions):
            out.append({
                "id": ri,
                "code": r.code,
                "name": r.name,
                "continent": r.continent,
                "value": r.value,
                "area_km2": round_half_up(r.area_km2),
                "population": round_half_up(r.population),
                "label": [r.label % WIDTH, r.label // WIDTH],
                "cells": len(r.cells),
            })
        return out

    def edge_records(self):
        out = []
        for key in sorted(self.edges):
            a, b = key
            km = round_half_up(self.label_dist(a, b))
            out.append({"a": a, "b": b, "kind": self.edges[key], "length_km": max(MIN_EDGE_KM, km)})
        return out

    def bitmap_rows(self):
        rows = []
        grid = self.grid
        for y in range(HEIGHT):
            base = y * WIDTH
            runs = []
            cur = None
            length = 0
            for x in range(WIDTH):
                v = grid[base + x]
                v = OCEAN if v == NONE else v
                if v == cur and length < 0xFFFF:
                    length += 1
                else:
                    if cur is not None:
                        runs.append((cur, length))
                    cur = v
                    length = 1
            runs.append((cur, length))
            rows.append(runs)
        return rows


def neighbors4_wrap(idx):
    y = idx // WIDTH
    x = idx % WIDTH
    out = [y * WIDTH + (x + 1) % WIDTH, y * WIDTH + (x - 1) % WIDTH]
    if y > 0:
        out.append(idx - WIDTH)
    if y + 1 < HEIGHT:
        out.append(idx + WIDTH)
    return out


def neighbors8(idx):
    y = idx // WIDTH
    x = idx % WIDTH
    out = []
    for dy in (-1, 0, 1):
        ny = y + dy
        if ny < 0 or ny >= HEIGHT:
            continue
        for dx in (-1, 0, 1):
            if dx == 0 and dy == 0:
                continue
            nx = x + dx
            if nx < 0 or nx >= WIDTH:
                continue
            out.append(ny * WIDTH + nx)
    return out


# ---------------------------------------------------------------------------
# Validation and writing
# ---------------------------------------------------------------------------

def validate(regions, edges, rows, n_components, grid):
    errors = []
    n = len(regions)
    if not (MIN_REGIONS <= n <= MAX_REGIONS):
        errors.append("region count %d outside %d..=%d" % (n, MIN_REGIONS, MAX_REGIONS))
    if n > 0xFFFE:
        errors.append("too many regions for u16 ids")
    codes = set()
    prev = None
    for i, r in enumerate(regions):
        if r["id"] != i:
            errors.append("non-contiguous id at %d" % i)
        if not CODE_RE.match(r["code"]):
            errors.append("bad code %r" % r["code"])
        if r["code"] in codes:
            errors.append("duplicate code %r" % r["code"])
        codes.add(r["code"])
        if prev is not None and not (prev < r["code"]):
            errors.append("regions not sorted by code at %r" % r["code"])
        prev = r["code"]
        if r["continent"] not in CONTINENTS:
            errors.append("%s: bad continent %r" % (r["code"], r["continent"]))
        if r["cells"] < 1:
            errors.append("%s: no cells" % r["code"])
        if not (1 <= r["value"] <= MAX_VALUE):
            errors.append("%s: value %d out of range" % (r["code"], r["value"]))
        if not r["name"] or not all(32 <= ord(ch) < 127 for ch in r["name"]):
            errors.append("%s: name must be non-empty printable ASCII: %r" % (r["code"], r["name"]))
    total = sum(r["value"] for r in regions)
    for r in regions:
        if total > 0 and r["value"] > MAX_VALUE_SHARE * total:
            errors.append("%s: value %d exceeds %.0f%% of total %d" % (
                r["code"], r["value"], MAX_VALUE_SHARE * 100, total))
    seen = set()
    for e in edges:
        if not (0 <= e["a"] < e["b"] < n):
            errors.append("bad edge %r" % (e,))
        key = (e["a"], e["b"])
        if key in seen:
            errors.append("duplicate edge %r" % (key,))
        seen.add(key)
        if e["kind"] not in ("land", "sea"):
            errors.append("bad edge kind %r" % (e,))
        if e["length_km"] < MIN_EDGE_KM:
            errors.append("edge too short %r" % (e,))
    if n_components != 1:
        errors.append("graph has %d components" % n_components)
    if len(rows) != HEIGHT:
        errors.append("bitmap has %d rows" % len(rows))
    counted = [0] * n
    for y, runs in enumerate(rows):
        s = 0
        for v, length in runs:
            s += length
            if length <= 0:
                errors.append("row %d: empty run" % y)
            if v != OCEAN:
                if v >= n:
                    errors.append("row %d: bad region id %d" % (y, v))
                else:
                    counted[v] += length
        if s != WIDTH:
            errors.append("row %d: run lengths sum to %d, not %d" % (y, s, WIDTH))
        if len(runs) > 0xFFFF:
            errors.append("row %d: too many runs" % y)
    for i, r in enumerate(regions):
        if counted[i] != r["cells"]:
            errors.append("%s: bitmap has %d cells, json says %d" % (r["code"], counted[i], r["cells"]))
        lx, ly = r["label"]
        if not (0 <= lx < WIDTH and 0 <= ly < HEIGHT):
            errors.append("%s: label out of raster" % r["code"])
        elif grid[ly * WIDTH + lx] != i:
            errors.append("%s: label cell (%d, %d) is not inside the region" % (r["code"], lx, ly))
    return errors


def write_regions_json(path, regions, edges):
    def dumps(obj):
        return json.dumps(obj, ensure_ascii=True, separators=(", ", ": "))
    lines = ["{"]
    lines.append('  "version": 1,')
    lines.append('  "raster": ' + dumps({
        "width": WIDTH, "height": HEIGHT, "cells_per_degree": CPD,
        "lat_top": LAT_TOP, "lon_left": LON_LEFT}) + ",")
    lines.append('  "regions": [')
    for i, r in enumerate(regions):
        lines.append("    " + dumps(r) + ("," if i + 1 < len(regions) else ""))
    lines.append("  ],")
    lines.append('  "edges": [')
    for i, e in enumerate(edges):
        lines.append("    " + dumps(e) + ("," if i + 1 < len(edges) else ""))
    lines.append("  ]")
    lines.append("}")
    data = "\n".join(lines) + "\n"
    with open(path, "w", encoding="ascii", newline="\n") as fh:
        fh.write(data)
    return len(data)


def write_bitmap(path, rows):
    parts = [struct.pack("<4sHHH", b"TBMP", 1, WIDTH, HEIGHT)]
    for runs in rows:
        parts.append(struct.pack("<H", len(runs)))
        for v, length in runs:
            parts.append(struct.pack("<HH", v, length))
    data = b"".join(parts)
    with open(path, "wb") as fh:
        fh.write(data)
    return len(data)


# ---------------------------------------------------------------------------
# Reporting and previews
# ---------------------------------------------------------------------------

def print_summary(p, regions, edges):
    n = len(regions)
    land = sum(1 for e in edges if e["kind"] == "land")
    sea = len(edges) - land
    total = sum(r["value"] for r in regions)
    info("")
    info("=== summary ===")
    info("regions: %d   edges: %d (land %d, sea %d)   total value: %d" % (n, len(edges), land, sea, total))
    vals = sorted(r["value"] for r in regions)
    info("value: min %d  median %d  mean %.2f  max %d  (max share %.2f%%)" % (
        vals[0], vals[len(vals) // 2], float(total) / n, vals[-1], 100.0 * vals[-1] / total))
    hist = {}
    for v in vals:
        hist[v] = hist.get(v, 0) + 1
    info("value histogram: " + "  ".join("%d:%d" % (k, hist[k]) for k in sorted(hist)))
    cont = {}
    for r in regions:
        c = cont.setdefault(r["continent"], [0, 0])
        c[0] += 1
        c[1] += r["value"]
    info("by continent: " + "  ".join("%s %d regions / value %d" % (k, cont[k][0], cont[k][1]) for k in sorted(cont)))
    deg = [0] * n
    for e in edges:
        deg[e["a"]] += 1
        deg[e["b"]] += 1
    info("degree: min %d  max %d  mean %.2f" % (min(deg), max(deg), 2.0 * len(edges) / n))
    by_val = sorted(regions, key=lambda r: (-r["value"], r["code"]))[:20]
    info("top by value: " + ", ".join("%s=%d" % (r["code"], r["value"]) for r in by_val))
    by_cells = sorted(regions, key=lambda r: (-r["cells"], r["code"]))[:15]
    info("top by cells: " + ", ".join("%s=%d" % (r["code"], r["cells"]) for r in by_cells))
    small = sorted(regions, key=lambda r: (r["cells"], r["code"]))[:15]
    info("smallest by cells: " + ", ".join("%s=%d" % (r["code"], r["cells"]) for r in small))
    splits = {}
    for r in p.regions:
        if r.split:
            splits.setdefault(r.unit.adm0, []).append(r.code)
    info("split regions:")
    for k in sorted(splits):
        info("  %s -> %s" % (k, ", ".join(sorted(splits[k]))))
    info("merges (%d):" % len(p.report_merges))
    for m in p.report_merges:
        info("  " + m)
    info("drops (%d):" % len(p.report_drops))
    for m in p.report_drops:
        info("  " + m)
    if p.report_stamps:
        info("stamped label cells:")
        for m in p.report_stamps:
            info("  " + m)
    if p.report_skipped_lanes:
        info("curated sea lanes skipped (already land neighbors): " + ", ".join(p.report_skipped_lanes))
    if p.report_auto_lanes:
        info("automatic sea lanes:")
        for m in p.report_auto_lanes:
            info("  " + m)


def print_world_preview(p, cols=180, rows=70):
    bx = WIDTH // cols
    by = HEIGHT // rows
    grid = p.grid
    info("")
    info("=== world preview (%dx%d, first letter of region code) ===" % (cols, rows))
    for ry in range(rows):
        line = []
        for rx in range(cols):
            counts = {}
            land = 0
            for y in range(ry * by, ry * by + by):
                base = y * WIDTH
                for x in range(rx * bx, rx * bx + bx):
                    v = grid[base + x]
                    if v != NONE:
                        land += 1
                        counts[v] = counts.get(v, 0) + 1
            if land * 4 < bx * by:
                line.append("." if land > 0 else " ")
            else:
                best = sorted(counts.items(), key=lambda kv: (-kv[1], kv[0]))[0][0]
                line.append(p.regions[best].code[0])
        info("".join(line).rstrip())


def print_box_preview(p, lon0, lon1, lat0, lat1, max_cols=160):
    x0 = max(0, int(math.floor((lon0 - LON_LEFT) * CPD)))
    x1 = min(WIDTH, int(math.ceil((lon1 - LON_LEFT) * CPD)))
    y0 = max(0, int(math.floor((LAT_TOP - lat1) * CPD)))
    y1 = min(HEIGHT, int(math.ceil((LAT_TOP - lat0) * CPD)))
    step = max(1, int(math.ceil((x1 - x0) / float(max_cols))))
    ystep = step * 2
    symbols = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789@$%&*+=?<>"
    assigned = {}
    grid = p.grid
    out = []
    for y in range(y0, y1, ystep):
        line = []
        for x in range(x0, x1, step):
            v = grid[y * WIDTH + x]
            if v == NONE:
                line.append(" ")
                continue
            if v not in assigned:
                assigned[v] = symbols[len(assigned) % len(symbols)]
            ch = assigned[v]
            if p.regions[v].label == y * WIDTH + x:
                ch = "#"
            line.append(ch)
        out.append("".join(line).rstrip())
    info("")
    info("=== box preview lon %s..%s lat %s..%s (step %d) ===" % (lon0, lon1, lat0, lat1, step))
    for l in out:
        info(l)
    legend = sorted(assigned.items(), key=lambda kv: kv[1])
    info("legend: " + "  ".join("%s=%s" % (s, p.regions[v].code) for v, s in legend))


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main(argv):
    here = os.path.dirname(os.path.abspath(__file__))
    ap = argparse.ArgumentParser(description="Natural Earth -> Territorial CLI map assets")
    ap.add_argument("--input", help="Natural Earth 50m admin-0 countries GeoJSON")
    ap.add_argument("--download", action="store_true",
                    help="fetch the GeoJSON into mapgen/cache/ (if not cached) and use it")
    ap.add_argument("--out", required=True, help="assets directory to write into")
    ap.add_argument("--curation", default=os.path.join(here, "curation.json"))
    ap.add_argument("--preview", action="store_true", help="print an ASCII world preview")
    ap.add_argument("--preview-box", nargs=4, type=float, action="append", default=[],
                    metavar=("LON0", "LON1", "LAT0", "LAT1"),
                    help="print a detailed ASCII preview of a lon/lat box (repeatable)")
    ap.add_argument("--dry-run", action="store_true", help="validate but do not write assets")
    args = ap.parse_args(argv)

    if args.download:
        path = download(os.path.join(here, "cache"))
    elif args.input:
        path = args.input
    else:
        ap.error("either --input or --download is required")
        return 2

    try:
        geo = load_json(path)
        cur = load_json(args.curation)
        p = Pipeline(geo, cur)
        p.build_units()
        p.rasterize()
        p.build_regions()
        p.assign_populations()
        p.stamp_and_prune()
        p.compact()
        p.compute_metrics()
        p.compute_values()
        p.compute_land_edges()
        p.compute_sea_edges()
        regions = p.region_records()
        edges = p.edge_records()
        rows = p.bitmap_rows()
        n_comp = len(p.components())
    except MapgenError as exc:
        print("mapgen: error: %s" % exc, file=sys.stderr)
        return 1

    print_summary(p, regions, edges)
    if args.preview:
        print_world_preview(p)
    for box in args.preview_box:
        print_box_preview(p, box[0], box[1], box[2], box[3])

    errors = validate(regions, edges, rows, n_comp, p.grid)
    if errors:
        print("", file=sys.stderr)
        print("mapgen: VALIDATION FAILED (%d problems):" % len(errors), file=sys.stderr)
        for e in errors:
            print("  - " + e, file=sys.stderr)
        return 1
    info("")
    info("validation: OK")
    if args.dry_run:
        info("dry run: nothing written")
        return 0
    os.makedirs(args.out, exist_ok=True)
    jpath = os.path.join(args.out, "world_regions.json")
    bpath = os.path.join(args.out, "world_bitmap.bin")
    jsize = write_regions_json(jpath, regions, edges)
    bsize = write_bitmap(bpath, rows)
    info("wrote %s (%d bytes)" % (jpath, jsize))
    info("wrote %s (%d bytes)" % (bpath, bsize))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
