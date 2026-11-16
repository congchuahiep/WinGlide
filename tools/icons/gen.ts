//! Regenerates the Windows icon assets from the SVG sources:
//!
//!     assets/tray-light.ico   tray glyph, 7 entries (16..48), black
//!     assets/tray-dark.ico    same artwork painted white
//!     assets/app.ico          application icon, 10 entries (16..256)
//!
//! The tray glyph is drawn on a 16-unit grid with 1-unit strokes, so only the
//! sizes that are whole multiples of 16 land on the pixel grid when rasterized.
//! Sizes that were optically tuned in Figma live in `assets/overrides/` and win
//! over the vector render; the remaining sizes are rendered from the SVG and
//! keep their anti-aliased edges.
//!
//!     tray size   16  20  24  28  32  40  48
//!     source      ov  ov  ov  svg ov  svg ov
//!
//! The dark tray variant reuses the alpha channel of the same artwork and paints
//! it white, so one set of source files serves both taskbar themes.

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { Resvg } from "@resvg/resvg-js";
import { PNG } from "pngjs";

const ASSETS = join(import.meta.dir, "..", "..", "assets");
const MSIX_ASSETS = join(import.meta.dir, "..", "..", "packaging", "Assets");
const TRAY_SIZES = [16, 20, 24, 28, 32, 40, 48];
const APP_SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256];
/// Sizes Windows asks for on its own, resolved by the `targetsize-<n>` suffix in the
/// file name. These are drawn edge to edge, unlike the plated tiles below.
const MSIX_TARGET_SIZES = [16, 20, 24, 30, 32, 36, 40, 44, 48, 60, 64, 80, 96, 256];

interface RgbaImage {
	width: number;
	height: number;
	data: Buffer;
}

interface IconEntry {
	width: number;
	height: number;
	payload: Buffer;
}

function decodePng(buffer: Buffer): RgbaImage {
	const png = PNG.sync.read(buffer);
	return { width: png.width, height: png.height, data: png.data };
}

function encodePng(image: RgbaImage): Buffer {
	const png = new PNG({ width: image.width, height: image.height });
	image.data.copy(png.data);
	return PNG.sync.write(png);
}

/// Centres `inner` on a transparent canvas of the requested tile size.
function placeOnTile(inner: RgbaImage, width: number, height: number): RgbaImage {
	const data = Buffer.alloc(width * height * 4);
	const offsetX = Math.floor((width - inner.width) / 2);
	const offsetY = Math.floor((height - inner.height) / 2);
	for (let y = 0; y < inner.height; y++) {
		for (let x = 0; x < inner.width; x++) {
			const src = (y * inner.width + x) * 4;
			const dst = ((offsetY + y) * width + offsetX + x) * 4;
			data[dst] = inner.data[src];
			data[dst + 1] = inner.data[src + 1];
			data[dst + 2] = inner.data[src + 2];
			data[dst + 3] = inner.data[src + 3];
		}
	}
	return { width, height, data };
}

function renderPng(svg: string, size: number): Buffer {
	return new Resvg(svg, { fitTo: { mode: "width", value: size } })
		.render()
		.asPng();
}

/// Replaces RGB with a single color, keeping the alpha channel untouched. The
/// tray artwork is monochrome, so the alpha channel carries the whole shape.
function tint(image: RgbaImage, r: number, g: number, b: number): RgbaImage {
	const data = Buffer.from(image.data);
	for (let i = 0; i < data.length; i += 4) {
		data[i] = r;
		data[i + 1] = g;
		data[i + 2] = b;
	}
	return { width: image.width, height: image.height, data };
}

/// Encodes one ICO entry as a 32bpp bottom-up DIB plus an all-zero AND mask.
/// The AND mask is kept for code paths that still read it; the visible shape
/// comes from the alpha channel.
function bmpEntry(image: RgbaImage): IconEntry {
	const { width, height, data } = image;
	const xor = Buffer.alloc(width * height * 4);
	for (let y = 0; y < height; y++) {
		for (let x = 0; x < width; x++) {
			const src = (y * width + x) * 4;
			const dst = ((height - 1 - y) * width + x) * 4;
			xor[dst] = data[src + 2]; // B
			xor[dst + 1] = data[src + 1]; // G
			xor[dst + 2] = data[src]; // R
			xor[dst + 3] = data[src + 3]; // A
		}
	}
	const andMask = Buffer.alloc(Math.ceil(width / 32) * 4 * height);

	const header = Buffer.alloc(40);
	header.writeUInt32LE(40, 0); // biSize
	header.writeInt32LE(width, 4); // biWidth
	header.writeInt32LE(height * 2, 8); // biHeight (XOR + AND mask)
	header.writeUInt16LE(1, 12); // biPlanes
	header.writeUInt16LE(32, 14); // biBitCount
	header.writeUInt32LE(0, 16); // biCompression
	header.writeUInt32LE(xor.length + andMask.length, 20); // biSizeImage

	return { width, height, payload: Buffer.concat([header, xor, andMask]) };
}

/// Large entries are stored as PNG: same pixels, a fraction of the DIB size.
/// Windows has supported PNG-compressed icons since Vista.
function pngEntry(png: Buffer, size: number): IconEntry {
	return { width: size, height: size, payload: png };
}

function ico(entries: IconEntry[]): Buffer {
	const header = Buffer.alloc(6);
	header.writeUInt16LE(0, 0); // reserved
	header.writeUInt16LE(1, 2); // type: icon
	header.writeUInt16LE(entries.length, 4);

	const directory = Buffer.alloc(16 * entries.length);
	let offset = header.length + directory.length;
	entries.forEach((entry, index) => {
		const at = index * 16;
		directory[at] = entry.width === 256 ? 0 : entry.width;
		directory[at + 1] = entry.height === 256 ? 0 : entry.height;
		directory[at + 2] = 0; // color palette
		directory[at + 3] = 0; // reserved
		directory.writeUInt16LE(1, at + 4); // planes
		directory.writeUInt16LE(32, at + 6); // bit count
		directory.writeUInt32LE(entry.payload.length, at + 8);
		directory.writeUInt32LE(offset, at + 12);
		offset += entry.payload.length;
	});

	return Buffer.concat([header, directory, ...entries.map((e) => e.payload)]);
}

/// Returns the raw PNG for one size: the hand-tuned override when it exists,
/// otherwise a render of the given SVG. Fails if the override has the wrong size.
function sourcePng(
	svg: string,
	name: string,
	size: number,
): { png: Buffer; source: string } {
	const overridePath = join(ASSETS, "overrides", `${name}-${size}.png`);
	if (existsSync(overridePath)) {
		const png = readFileSync(overridePath);
		const image = decodePng(png);
		if (image.width !== size || image.height !== size) {
			throw new Error(
				`${overridePath}: expected ${size}x${size}, got ${image.width}x${image.height}`,
			);
		}
		return { png, source: "override" };
	}
	return { png: renderPng(svg, size), source: "svg" };
}

const traySvg = readFileSync(join(ASSETS, "tray-icon.svg"), "utf8");
const trayReport: string[] = [];
const trayImages: RgbaImage[] = [];
for (const size of TRAY_SIZES) {
	const { png, source } = sourcePng(traySvg, "tray", size);
	trayImages.push(tint(decodePng(png), 0, 0, 0));
	trayReport.push(`  ${String(size).padStart(2)}px  ${source.padEnd(8)}`);
}

writeFileSync(join(ASSETS, "tray-light.ico"), ico(trayImages.map(bmpEntry)));
writeFileSync(
	join(ASSETS, "tray-dark.ico"),
	ico(trayImages.map((image) => bmpEntry(tint(image, 255, 255, 255)))),
);

const appSvg = readFileSync(join(ASSETS, "app-icon.svg"), "utf8");
const appReport: string[] = [];
const appEntries: IconEntry[] = [];
for (const size of APP_SIZES) {
	const { png, source } = sourcePng(appSvg, "app", size);
	appEntries.push(size <= 64 ? bmpEntry(decodePng(png)) : pngEntry(png, size));
	appReport.push(`  ${String(size).padStart(3)}px  ${source.padEnd(8)}`);
}
writeFileSync(join(ASSETS, "app.ico"), ico(appEntries));

console.log(`tray icons written:\n${trayReport.join("\n")}`);
console.log(`app icon written:\n${appReport.join("\n")}`);

// --- MSIX package assets ----------------------------------------------------
//
// The manifest points at plated tiles (Square44x44Logo, Square150x150Logo,
// Wide310x150Logo, StoreLogo, SplashScreen) and Windows resolves extra sizes on its
// own from `*.targetsize-<n>.png`. Target sizes are drawn edge to edge and reuse the
// app icon art, so hand-tuned overrides apply there; the plated tiles centre the
// artwork with breathing room.

mkdirSync(MSIX_ASSETS, { recursive: true });
const msixReport: string[] = [];

function writeTile(name: string, width: number, height: number, inner: number): void {
	const icon = decodePng(renderPng(appSvg, inner));
	const tile = placeOnTile(icon, width, height);
	writeFileSync(join(MSIX_ASSETS, name), encodePng(tile));
	msixReport.push(`  ${name.padEnd(36)} ${width}x${height} (icon ${inner}px)`);
}

writeTile("Square44x44Logo.png", 44, 44, 38);
writeTile("Square150x150Logo.png", 150, 150, 105);
writeTile("Wide310x150Logo.png", 310, 150, 104);
writeTile("StoreLogo.png", 50, 50, 43);
writeTile("SplashScreen.png", 620, 300, 160);

for (const size of MSIX_TARGET_SIZES) {
	const { png, source } = sourcePng(appSvg, "app", size);
	writeFileSync(join(MSIX_ASSETS, `Square44x44Logo.targetsize-${size}.png`), png);
	// Taskbar and Start draw the logo without a plate only when an `altform-unplated`
	// variant exists; without it Windows paints a square in the manifest's
	// BackgroundColor (transparent lets it pick a colour from the logo).
	writeFileSync(join(MSIX_ASSETS, `Square44x44Logo.targetsize-${size}_altform-unplated.png`), png);
	writeFileSync(join(MSIX_ASSETS, `Square44x44Logo.targetsize-${size}_altform-lightunplated.png`), png);
	msixReport.push(
		`  Square44x44Logo.targetsize-${String(size).padEnd(3)} ${size}x${size} (${source})`,
	);
}

console.log(`msix assets written:\n${msixReport.join("\n")}`);
