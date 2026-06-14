# Mobile-First + PWA Webview Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the mobile dashboard installable as a first-class home-screen webview (manifest + meta + icons) and document mobile as the canonical experience.

**Architecture:** Pure static-asset additions to the existing `server/static/` SPA (served by `ServeDir`), plus a docs change. No service worker, no CSS/layout change, no routing change, no desktop degradation. Icons are rasterized from the existing lightning-bolt motif by a stdlib-only Python generator (no SVG rasterizer is installed).

**Tech Stack:** Static HTML/JSON/PNG under `server/static/`; Python 3 stdlib (`zlib`, `struct`) for icon generation; Rust/Axum `ServeDir` (unchanged) serves the assets.

**Spec:** `docs/superpowers/specs/2026-06-14-mobile-first-pwa-design.md`

---

## Worktree & branch (already created)

Work happens in the sibling worktree on a feature branch — never on `main`.

```bash
# Already created; recreate only if missing:
git worktree add ../ctc_server-mobile_first_pwa -b feature/mobile_first_pwa main
cd ../ctc_server-mobile_first_pwa
git rev-parse --abbrev-ref HEAD   # expect: feature/mobile_first_pwa
```

**Rebase bracket:** branched off **local `main`** @ `0a853bf` (origin/main is ~22 commits stale and lacks v0.4.0 + the price-rollover fix). Before starting and before handing back, rebase onto local `main`:

```bash
git rebase main      # NOT origin/main — origin is stale; pushing stays the user's call
```

## Project guardrails (verbatim from CLAUDE.md)

**Pre-Commit Checklist** — before every commit:
- [ ] `cargo fmt` - Code is formatted
- [ ] `cargo clippy --all-targets -- -W clippy::pedantic` - Zero warnings
- [ ] `cargo test --all-targets` - All tests pass
- [ ] `cargo tarpaulin --all-targets --workspace` - Coverage ≥ 90%
- [ ] Commit message follows guidelines (≤50 chars, imperative verb)

**Git Operation Policy:** never edit or commit while HEAD is `main`. Allowed on a `feature/<name>` branch inside a worktree: `git worktree add/remove`, `git switch -c`, `git fetch`, `git rebase origin/main`, `git add`, `git commit`. Reserved for the user (do NOT run): `git push` (any form), `git merge`/squash-merge into `main`, destructive history rewrites.

**Commit messages:** subject ≤50 chars, imperative verb (Add/Fix/Update/...), no trailing period, no body unless complex, no attribution.

> Note: this change is static-asset + docs only — it does not touch Rust source, so `cargo` gates will be unchanged from the green baseline (462 tests). Run `cargo fmt`/clippy/test once at the end to confirm the baseline is intact rather than per static-file commit.

---

## File structure

| File | Responsibility | Action |
|---|---|---|
| `tools/gen_icons.py` | Stdlib PNG icon generator from the bolt motif | Create |
| `server/static/icon-192.png` | Manifest icon (maskable) | Create (generated) |
| `server/static/icon-512.png` | Manifest icon (maskable) | Create (generated) |
| `server/static/icon-180.png` | iOS `apple-touch-icon` | Create (generated) |
| `server/static/manifest.webmanifest` | PWA install metadata | Create |
| `server/static/index.html` | Link manifest + theme/apple meta + apple-touch-icon | Modify (`<head>`, after the favicon `<link>`) |
| `CLAUDE.md` | Mobile-first priority note | Modify (Web Dashboard section) |

---

## Task 1: Icon generator + PNG icons

**Files:**
- Create: `tools/gen_icons.py`
- Create (generated): `server/static/icon-192.png`, `server/static/icon-512.png`, `server/static/icon-180.png`

- [ ] **Step 1: Write the generator**

Create `tools/gen_icons.py`:

```python
#!/usr/bin/env python3
"""Generate PWA app icons (dark square + orange lightning bolt) with stdlib only.

No SVG rasterizer is installed on the dev box, so we rasterize the bolt polygon
directly (ray-cast fill, 2x supersampled for anti-aliasing) and encode PNGs via
zlib. Run from the repo/worktree root:  python3 tools/gen_icons.py
"""
import zlib
import struct
import os

BG = (0x14, 0x16, 0x1A)   # dark theme --bg (oklch(0.16 0.005 250))
FG = (0xF9, 0x73, 0x16)   # bolt orange (existing favicon stroke)
# Lightning-bolt vertices in a 24x24 viewBox (matches index.html favicon).
POLY = [(13, 2), (3, 14), (12, 14), (11, 22), (21, 10), (12, 10)]
SCALE = 0.62              # bolt fills ~62% of the square (maskable safe zone)
SS = 2                    # supersampling factor (anti-aliasing)
OUT = "server/static"
SIZES = {"icon-192.png": 192, "icon-512.png": 512, "icon-180.png": 180}


def inside(px, py, poly):
    """Even-odd ray-cast point-in-polygon test."""
    c = False
    j = len(poly) - 1
    for i in range(len(poly)):
        xi, yi = poly[i]
        xj, yj = poly[j]
        if ((yi > py) != (yj > py)) and (
            px < (xj - xi) * (py - yi) / (yj - yi) + xi
        ):
            c = not c
        j = i
    return c


def png_bytes(n, rows):
    """Encode square RGB image (rows: list of bytearray len n*3) as PNG."""
    raw = bytearray()
    for row in rows:
        raw.append(0)            # filter type 0 (None)
        raw.extend(row)

    def chunk(typ, data):
        return (
            struct.pack(">I", len(data)) + typ + data
            + struct.pack(">I", zlib.crc32(typ + data) & 0xFFFFFFFF)
        )

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", n, n, 8, 2, 0, 0, 0))  # 8-bit RGB
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )


def gen(n, path):
    m = n * SS
    s = (m * SCALE) / 24.0
    t = m / 2.0 - 12.0 * s        # center the bolt's (12,12) bbox centre
    k = SS * SS
    rows = []
    for y in range(n):
        row = bytearray()
        for x in range(n):
            r = g = b = 0
            for sy in range(SS):
                for sx in range(SS):
                    vx = (x * SS + sx + 0.5 - t) / s
                    vy = (y * SS + sy + 0.5 - t) / s
                    col = FG if inside(vx, vy, POLY) else BG
                    r += col[0]
                    g += col[1]
                    b += col[2]
            row += bytes((r // k, g // k, b // k))
        rows.append(row)
    with open(path, "wb") as f:
        f.write(png_bytes(n, rows))
    print(f"wrote {path} ({n}x{n})")


if __name__ == "__main__":
    os.makedirs(OUT, exist_ok=True)
    for name, size in SIZES.items():
        gen(size, os.path.join(OUT, name))
```

- [ ] **Step 2: Run the generator**

Run (from worktree root): `python3 tools/gen_icons.py`
Expected output (512 may take ~30–60s; it is a one-off):
```
wrote server/static/icon-192.png (192x192)
wrote server/static/icon-512.png (512x512)
wrote server/static/icon-180.png (180x180)
```

- [ ] **Step 3: Verify icon dimensions (the "test")**

Run:
```bash
python3 - <<'EOF'
import struct
for name, exp in [("icon-192.png", 192), ("icon-512.png", 512), ("icon-180.png", 180)]:
    with open(f"server/static/{name}", "rb") as f:
        f.read(16)
        w, h = struct.unpack(">II", f.read(8))
    assert (w, h) == (exp, exp), f"{name}: {w}x{h} != {exp}x{exp}"
    print(f"{name}: {w}x{h} OK")
EOF
```
Expected:
```
icon-192.png: 192x192 OK
icon-512.png: 512x512 OK
icon-180.png: 180x180 OK
```

- [ ] **Step 4: Eyeball the icons**

Open `server/static/icon-512.png` (e.g. `code server/static/icon-512.png`). Expected: an orange lightning bolt, centred, on a dark near-black square, bolt within the central ~62%.

- [ ] **Step 5: Commit**

```bash
git add tools/gen_icons.py server/static/icon-192.png server/static/icon-512.png server/static/icon-180.png
git commit -m "Add PWA icon generator and app icons"
```

---

## Task 2: Web app manifest

**Files:**
- Create: `server/static/manifest.webmanifest`

- [ ] **Step 1: Create the manifest**

Create `server/static/manifest.webmanifest`:

```json
{
  "name": "CTC Heating",
  "short_name": "CTC",
  "start_url": "/static/index.html",
  "scope": "/static/",
  "display": "standalone",
  "background_color": "#14161a",
  "theme_color": "#14161a",
  "icons": [
    { "src": "icon-192.png", "sizes": "192x192", "type": "image/png", "purpose": "any maskable" },
    { "src": "icon-512.png", "sizes": "512x512", "type": "image/png", "purpose": "any maskable" }
  ]
}
```

- [ ] **Step 2: Verify it is valid JSON and icon srcs resolve (the "test")**

Run:
```bash
python3 - <<'EOF'
import json, os
m = json.load(open("server/static/manifest.webmanifest"))
for key in ("name", "short_name", "start_url", "scope", "display", "icons"):
    assert key in m, f"missing key: {key}"
for ic in m["icons"]:
    p = os.path.join("server/static", ic["src"])
    assert os.path.exists(p), f"icon file missing: {p}"
print("manifest: valid JSON, all icon files present")
EOF
```
Expected: `manifest: valid JSON, all icon files present`

- [ ] **Step 3: Commit**

```bash
git add server/static/manifest.webmanifest
git commit -m "Add web app manifest"
```

---

## Task 3: Link manifest + PWA meta in index.html

**Files:**
- Modify: `server/static/index.html` (`<head>`, immediately after the existing `<link rel="icon" ...>` line, currently line 7)

- [ ] **Step 1: Insert the PWA head tags**

In `server/static/index.html`, find the existing favicon line:

```html
<link rel="icon" href="data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='%23f97316' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'><polygon points='13 2 3 14 12 14 11 22 21 10 12 10 13 2'/></svg>" />
```

Insert these lines immediately **after** it:

```html
<link rel="manifest" href="manifest.webmanifest" />
<meta name="theme-color" media="(prefers-color-scheme: dark)" content="#14161a" />
<meta name="theme-color" media="(prefers-color-scheme: light)" content="#fbfbf9" />
<meta name="apple-mobile-web-app-capable" content="yes" />
<meta name="apple-mobile-web-app-status-bar-style" content="black-translucent" />
<meta name="apple-mobile-web-app-title" content="CTC" />
<link rel="apple-touch-icon" href="icon-180.png" />
```

- [ ] **Step 2: Verify the tags are present (the "test")**

Run:
```bash
echo "manifest link : $(grep -c 'rel=\"manifest\"' server/static/index.html) (expect 1)"
echo "theme-color   : $(grep -c 'name=\"theme-color\"' server/static/index.html) (expect 2)"
echo "apple web-app : $(grep -c 'apple-mobile-web-app' server/static/index.html) (expect 3)"
echo "apple-touch   : $(grep -c 'rel=\"apple-touch-icon\"' server/static/index.html) (expect 1)"
```
Expected: `1`, `2`, `3`, `1` respectively.

- [ ] **Step 3: Commit**

```bash
git add server/static/index.html
git commit -m "Link manifest and PWA meta in index.html"
```

---

## Task 4: Document mobile-first priority in CLAUDE.md

**Files:**
- Modify: `CLAUDE.md` (end of the "### Web Dashboard (Static Files)" subsection, before "### Persistence")

- [ ] **Step 1: Add the mobile-first note**

In `CLAUDE.md`, locate the end of the `### Web Dashboard (Static Files)` subsection (it ends with the `style.css` bullet, just before `### Persistence`). Append this paragraph there:

```markdown

**Mobile is the canonical experience.** The compact mobile layout (the `≤480px`
`useIsNarrow` path) is the primary target: design and verify dashboard changes at a phone
viewport first. The wide desktop multi-column layout is a progressive enhancement that must
never regress the mobile view. The dashboard ships PWA install metadata
(`manifest.webmanifest`, theme-color + apple meta, `icon-*.png`) so the mobile view installs
as a standalone home-screen webview; there is intentionally **no service worker** (a
live-data dashboard must not serve stale cached readings).
```

- [ ] **Step 2: Verify the note landed (the "test")**

Run:
```bash
grep -c "Mobile is the canonical experience" CLAUDE.md   # expect 1
grep -c "no service worker" CLAUDE.md                     # expect 1
```
Expected: `1` and `1`.

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "Document mobile-first dashboard priority"
```

---

## Task 5: Final verification

- [ ] **Step 1: Confirm the Rust baseline is intact (static-only change)**

Run:
```bash
cargo fmt --check && echo "fmt OK"
cargo clippy --all-targets -- -W clippy::pedantic 2>&1 | grep -E "warning|error" || echo "clippy: clean"
cargo test --all-targets 2>&1 | grep "test result"
```
Expected: `fmt OK`, `clippy: clean`, and `test result: ok. 462 passed` (or current baseline; 0 failed).

- [ ] **Step 2: Offline asset sanity sweep**

Run:
```bash
ls -l server/static/manifest.webmanifest server/static/icon-192.png server/static/icon-512.png server/static/icon-180.png
python3 -m json.tool server/static/manifest.webmanifest > /dev/null && echo "manifest JSON OK"
```
Expected: four files listed (non-zero size), `manifest JSON OK`.

- [ ] **Step 3: Rebase bracket before handback**

```bash
git rebase main      # local main; resolve nothing expected (static-only, disjoint files)
git log --oneline -6
```

- [ ] **Step 4 (DEFERRED — requires deploy): live install verification**

The HTTP server cannot fully start on this dev box (no Modbus serial device at `/dev/ttyAMA4`), so live verification runs **after the user deploys to `ctc.lan`** (via the `ctc-deploy` skill). Post-deploy, from this box (read-only diagnosis), confirm:

```bash
curl -sI http://ctc.lan:3000/static/manifest.webmanifest   # 200; note Content-Type
curl -sI http://ctc.lan:3000/static/icon-192.png           # 200, image/png
```
Then load `http://ctc.lan:3000/` in the headless browser and confirm the `<head>` exposes the manifest link + both theme-color metas + the apple-touch-icon, the manifest parses with no console error, and (mobile viewport) the page is "Add to Home Screen"-able. **Acceptance note:** if `manifest.webmanifest` is served with a non-`application/manifest+json` content-type, browsers still parse a `rel="manifest"`-linked file; only add a server content-type override if a real browser rejects it (do not pre-build a speculative Rust route).

---

## Self-review

**Spec coverage:**
- Manifest (Component A) → Task 2. ✅
- index.html head meta/link (Component A) → Task 3. ✅
- Icons + no-rasterizer constraint (Component B) → Task 1 (stdlib generator refines the spec's "headless browser" method — same 3-PNG deliverable). ✅
- Mobile-first doc (Component C) → Task 4. ✅
- Verification (manifest 200, icons 200, head tags, headless check) → Task 5 (offline now; live deferred to post-deploy, with the device-absence reason stated). ✅
- Non-goals (no SW, no CSS/breakpoint/desktop change, no routing change) → respected; only files in the table are touched. ✅

**Placeholder scan:** none — every code/command step shows full content and expected output.

**Type/name consistency:** icon filenames (`icon-192/512/180.png`), `manifest.webmanifest`, color `#14161a`/`#fbfbf9`, and the `theme-color`/`apple-*` tag set are identical across Tasks 1–5 and the verify greps.

**Deviation flagged to user:** icon generation method (stdlib Python vs spec's headless browser) — same artifact; awaiting user OK at handoff.
