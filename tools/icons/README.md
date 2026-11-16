# Icon asset generator

Produces every icon Windows sees for WinGlide from the vector sources in
`assets/`: the `.ico` files embedded in the executable and the PNG tiles the
Microsoft Store package shows. The generated files are committed, so building the
app, the package and CI never need Bun; this tool only runs when the artwork
changes.

| Output                                   | Used by                                                          |
| ---------------------------------------- | ---------------------------------------------------------------- |
| `assets/tray-light.ico`, `tray-dark.ico` | the tray icon (black for a light taskbar, white for a dark one)  |
| `assets/app.ico`                         | the exe icon (embedded by `build.rs`) and the Settings title bar |
| `packaging/Assets/` (47 PNGs)            | MSIX tiles, Store logo and splash screen                         |

```bash
cd tools/icons
bun install
bun run gen
```

## How sizes are chosen

`bun gen.ts` builds `assets/tray-light.ico` and `assets/tray-dark.ico`, each with
seven entries (16, 20, 24, 28, 32, 40, 48 px) so Windows can pick the exact size
for the taskbar's DPI instead of downscaling a single bitmap.

For each size the generator rasterizes `assets/tray-icon.svg`. The glyph sits on a 16-unit grid with 1-unit strokes, so 16/32/48 land exactly on the pixel grid while 20/24/28/40 keep anti-aliased edges — a deliberate trade to keep a single source of truth for the artwork. There are no hand-tuned tray overrides checked in at the moment; add `assets/overrides/tray-<size>.png` only when one size needs its own art.

## The application icon

`assets/app.ico` holds ten entries (16, 20, 24, 32, 40, 48, 64, 96, 128, 256 px).
Entries up to 64 px are stored as bitmaps, larger ones as PNG: same pixels, a
fraction of the file size, and Windows has read PNG-compressed icons since Vista.
`build.rs` embeds this file into the executable; the tray reads the two tray ICOs
and the Settings window loads this one for its title bar.

## Package logos

The generator also writes the 47 PNGs in `packaging/Assets/`:

| File                                          | Size      | Artwork      |
| --------------------------------------------- | --------- | ------------ |
| `Square44x44Logo.png`                         | 44x44     | icon 38px    |
| `Square150x150Logo.png`                       | 150x150   | icon 105px   |
| `Wide310x150Logo.png`                         | 310x150   | icon 104px   |
| `StoreLogo.png`                               | 50x50     | icon 43px    |
| `SplashScreen.png`                            | 620x300   | icon 160px   |
| `Square44x44Logo.targetsize-<n>.png`          | 16..256   | edge to edge |
| `...targetsize-<n>_altform-unplated.png`      | same      | same         |
| `...targetsize-<n>_altform-lightunplated.png` | same      | same         |

The plated tiles centre the artwork with breathing room. `targetsize` covers the
sizes Windows asks for on its own (16, 20, 24, 30, 32, 36, 40, 44, 48, 60, 64,
80, 96, 256); the `_altform-unplated` spellings are how Start and the taskbar ask
for a logo *without* the coloured plate behind it. File names alone are not
enough - Windows resolves them through `resources.pri`, which is why
`packaging/build-msix.ps1` rebuilds that index; see
[`packaging/README.md`](../../packaging/README.md).

## Override requirements

Overrides are optional and currently unused for the tray glyph. When one is
added:
- Exact size in the filename and in the pixels (e.g. `tray-24.png` is 24×24);
  the generator stops with an error when they disagree, so a mistyped size cannot
  ship. Application overrides take the `app-` prefix.
- An override feeds every output of that size, so one hand-tuned `app-24.png`
  ends up in both `assets/app.ico` and `Square44x44Logo.targetsize-24.png`.
- Transparent background, a single solid colour, no effects.
- Edges aligned to whole pixels: no semi-transparent pixels along straight
  vertical or horizontal edges.

The dark variant is not authored separately: the generator keeps the alpha
channel of the same art and replaces RGB with white, so one set of files drives
both taskbar themes.

## Rules

- Do not edit the `.ico` files or `packaging/Assets/` by hand - the next run
  overwrites them. Change the SVG, an override PNG, or `gen.ts`.
- After regenerating, commit the output: build machines only need Rust.
- Keep the package asset names; the manifest and Windows' own logo lookup depend
  on them.
