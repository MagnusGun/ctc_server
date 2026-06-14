# Mobile-First + PWA Webview — Design

**Date:** 2026-06-14
**Branch:** `feature/mobile_first_pwa` (worktree `../ctc_server-mobile_first_pwa`, off local `main` @ `0a853bf`)
**Status:** Approved design — implementation plan pending.

## Context

The dashboard (`server/static/`) is **one responsive React SPA**: `index.html` loads the
`.jsx` files via in-browser `@babel/standalone` (no bundler/compile step). "Mobile" vs
"desktop" is conditional rendering driven by `useIsNarrow` (`max-width: 480px`, `hooks.jsx`)
plus media-query CSS in `styles.css`. A polished compact mobile layout already shipped on
`feature/mobile_compression`. The CSS is currently **desktop-first** (base = wide grids,
`max-width` queries compress down).

The goal "promote mobile to first-class, demote the normal dashboard" was scoped down to
**Approach 1: Priority-only + PWA**. Because the app is already responsive and the mobile
layout is already done, "first-class" here means *the experience we design/verify first and
that ships as a real installable webview* — not a CSS rewrite.

## Goals

1. Make the mobile dashboard installable as a first-class home-screen / webview app.
2. Document mobile as the canonical experience so future work is verified mobile-first and
   desktop can never dictate or regress it.

## Non-goals (explicitly out of scope)

- No CSS cascade inversion (desktop-first → mobile-first). Same pixels, high regression
  risk, no visible payoff — rejected as over-engineering for an already-responsive app.
- No breakpoint change (the `480px` boundary stays).
- No desktop degradation (desktop stays roomy multi-column; it is the *enhancement*).
- No service worker / offline cache — this is a live-data heating dashboard; cached stale
  temperatures/prices would mislead. Standalone install works without one.
- No routing change (`/` still redirects to `/static/index.html`).

## Design

### Component A — PWA install metadata

**New file `server/static/manifest.webmanifest`** (served by the existing
`ServeDir("/static")`, `server/src/main.rs:552`):

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

- `display: standalone` — keeps the OS status bar (a glanceable dashboard, not a game);
  `fullscreen` was rejected.
- Colors derived from the dark theme token `--bg: oklch(0.16 0.005 250)` ≈ `#14161a`.
  Exact hex to be confirmed against the rendered token at implementation.
- `start_url`/`scope` under `/static/` because that is where the app is actually served.

**Additions to `server/static/index.html` `<head>`** (it currently has none of these):

- `<link rel="manifest" href="manifest.webmanifest">`
- `<meta name="theme-color" media="(prefers-color-scheme: dark)" content="#14161a">`
- `<meta name="theme-color" media="(prefers-color-scheme: light)" content="#fbfbf9">`
  (the app ships a light theme too, `styles.css:75+`; light `--bg` ≈ `#fbfbf9`)
- `<meta name="apple-mobile-web-app-capable" content="yes">`
- `<meta name="apple-mobile-web-app-status-bar-style" content="black-translucent">`
- `<meta name="apple-mobile-web-app-title" content="CTC">`
- `<link rel="apple-touch-icon" href="icon-180.png">`

### Component B — App icons

Reuse the existing favicon motif: a lightning bolt (stroke `#f97316` orange, the inline SVG
in `index.html`) on a dark rounded-square background. Produce three committed PNGs in
`server/static/`:

- `icon-192.png`, `icon-512.png` — manifest, `purpose: "any maskable"` (keep the bolt inside
  the maskable safe zone, ~80% center).
- `icon-180.png` — iOS `apple-touch-icon`.

**Constraint:** no CLI SVG rasterizer is installed on the dev box (`rsvg-convert`,
`inkscape`, ImageMagick, `cairosvg` all absent). Generation method: render the bolt SVG in
the headless browser (Playwright, already available) at each pixel size and save the
screenshot as PNG. Fallback if that proves fiddly: install `librsvg2-bin`
(`rsvg-convert`) or `pip install cairosvg` for the one-off generation. The PNGs are
committed; no rasterizer is needed at runtime or build time afterward.

### Component C — Mobile-first priority (documentation)

Add a short subsection to `CLAUDE.md` under the Web Dashboard section stating:

> The mobile (compact `≤480px`) layout is the canonical experience. When changing the
> dashboard, design and verify it at a phone viewport first; the desktop multi-column
> layout is a progressive enhancement that must never regress the mobile view.

No runtime code change.

## Data flow

None new. The manifest and icons are static assets fetched by the browser; the existing
`ServeDir` serves them. Confirm `ServeDir` returns a sane `Content-Type` for
`.webmanifest` (`application/manifest+json` preferred; `text/plain` is tolerated by
browsers but should be checked).

## Verification

1. `GET /static/manifest.webmanifest` → 200, valid JSON, acceptable content-type.
2. `GET /static/icon-192.png`, `icon-512.png`, `icon-180.png` → 200, correct dimensions.
3. Headless browser at 390×844 (iPhone-ish): `index.html` `<head>` exposes the manifest
   link, both `theme-color` metas, and the `apple-touch-icon`; the manifest parses with no
   console errors. No auto-install prompt is expected (no service worker by design).
4. `cargo test --all-targets` + `cargo clippy --all-targets -- -W clippy::pedantic` still
   green (static-only change; Rust untouched aside from possibly a content-type tweak).

## Implementation notes

- All work in worktree `../ctc_server-mobile_first_pwa` on `feature/mobile_first_pwa`,
  branched off **local `main`** (origin/main is ~22 commits stale and lacks the v0.4.0 +
  price-rollover work; pushing remains the user's call).
- Independent of the unmerged `feature/mobile_compression` branch — manifest/meta/icons do
  not depend on its layout, so the two can land in either order.
- Pre-commit gate per `CLAUDE.md`: `cargo fmt`, clippy pedantic (zero warnings), tests pass.
- Commit subjects ≤50 chars, imperative; no body unless complex; no attribution.

## Resolved decisions

| Decision | Choice |
|---|---|
| App name / short name | "CTC Heating" / "CTC" |
| Display mode | `standalone` |
| Service worker | None |
| Icon source | Existing lightning-bolt motif, dark rounded-square bg |
| Icon generation | Headless-browser render (no CLI rasterizer present) |
| Branch base | Local `main` @ `0a853bf` |
