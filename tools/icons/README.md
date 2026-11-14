# Icon asset generator

Produces the Windows `.ico` files from the vector sources in `assets/`. The
generated files are committed, so building the app and CI never need Bun; this
tool only runs when the artwork changes.

```bash
cd tools/icons
bun install
bun run gen
```

## How sizes are chosen

`bun gen.ts` builds `assets/tray-light.ico` and `assets/tray-dark.ico`, each with
seven entries (16, 20, 24, 28, 32, 40, 48 px) so Windows can pick the exact size
for the taskbar's DPI instead of downscaling a single bitmap.

For each size the generator prefers `assets/overrides/tray-<size>.png` when it
exists, and otherwise rasterizes `assets/tray-icon.svg`. The overrides are the
sizes that were tuned by hand in Figma: the glyph sits on a 16-unit grid with
1-unit strokes, so only 16/32/48 are pixel-exact when rendered from the vector,
while 20/24/28/40 need their own art. At the moment 28 and 40 come from the SVG
and therefore keep anti-aliased edges — that is a deliberate trade for two rare
DPI settings.

## Override requirements

- Exact size in the filename and in the pixels (e.g. `tray-24.png` is 24×24).
- Transparent background, a single solid colour, no effects.
- Edges aligned to whole pixels: no semi-transparent pixels along straight
  vertical or horizontal edges.

The dark variant is not authored separately: the generator keeps the alpha
channel of the same art and replaces RGB with white, so one set of files drives
both taskbar themes.
