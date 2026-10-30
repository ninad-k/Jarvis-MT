// Draws the HUD orb as PNG app icons (no dependencies: raw pixels + zlib).
// Usage: pnpm icons   -> static/icons/*.png (committed; rerun only when the artwork changes).
import { deflateSync, crc32 } from 'node:zlib';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const out = join(dirname(fileURLToPath(import.meta.url)), '..', 'static', 'icons');
mkdirSync(out, { recursive: true });

const BG = [4, 7, 13]; // --color-void
const mix = (a, b, t) => a + (b - a) * Math.min(1, Math.max(0, t));
const smooth = (e0, e1, x) => {
	const t = Math.min(1, Math.max(0, (x - e0) / (e1 - e0)));
	return t * t * (3 - 2 * t);
};

/** Colour of the orb artwork at normalised position (x, y) in 0..1; `r0` is the orb radius. */
function pixel(x, y, r0) {
	const dx = x - 0.5;
	const dy = y - 0.5;
	const d = Math.hypot(dx, dy);
	let c = [...BG];
	// Faint vignette glow across the whole tile.
	const wash = Math.exp(-((d / 0.55) ** 2)) * 0.22;
	c = c.map((v, i) => mix(v, [14, 60, 92][i], wash));
	// Outer halo.
	const halo = d > r0 ? 0.55 * Math.exp(-(d - r0) / (r0 * 0.55)) : 0;
	c = c.map((v, i) => mix(v, [34, 211, 238][i], halo));
	// Ring.
	const ringD = Math.abs(d - r0 * 1.62);
	const ring = (1 - smooth(r0 * 0.012, r0 * 0.04, ringD)) * 0.55;
	c = c.map((v, i) => mix(v, [34, 211, 238][i], ring));
	// Core sphere.
	if (d <= r0 * 1.02) {
		const lx = (dx + r0 * 0.28) / r0;
		const ly = (dy + r0 * 0.32) / r0;
		const k = Math.min(1, Math.hypot(lx, ly) / 1.1);
		const core = k < 0.45
			? [mix(201, 34, k / 0.45), mix(248, 211, k / 0.45), mix(255, 238, k / 0.45)]
			: [mix(34, 10, (k - 0.45) / 0.55), mix(211, 63, (k - 0.45) / 0.55), mix(238, 134, (k - 0.45) / 0.55)];
		const edge = 1 - smooth(r0 * 0.97, r0 * 1.02, d);
		c = c.map((v, i) => mix(v, core[i], edge));
	}
	return c;
}

function png(size, r0) {
	const row = size * 4 + 1;
	const raw = Buffer.alloc(row * size);
	for (let y = 0; y < size; y++) {
		raw[y * row] = 0; // filter: none
		for (let x = 0; x < size; x++) {
			const c = pixel((x + 0.5) / size, (y + 0.5) / size, r0);
			const o = y * row + 1 + x * 4;
			raw[o] = Math.round(c[0]);
			raw[o + 1] = Math.round(c[1]);
			raw[o + 2] = Math.round(c[2]);
			raw[o + 3] = 255;
		}
	}
	const chunk = (type, data) => {
		const len = Buffer.alloc(4);
		len.writeUInt32BE(data.length);
		const body = Buffer.concat([Buffer.from(type), data]);
		const crc = Buffer.alloc(4);
		crc.writeUInt32BE(crc32(body) >>> 0);
		return Buffer.concat([len, body, crc]);
	};
	const ihdr = Buffer.alloc(13);
	ihdr.writeUInt32BE(size, 0);
	ihdr.writeUInt32BE(size, 4);
	ihdr[8] = 8; // bit depth
	ihdr[9] = 6; // RGBA
	return Buffer.concat([
		Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
		chunk('IHDR', ihdr),
		chunk('IDAT', deflateSync(raw, { level: 9 })),
		chunk('IEND', Buffer.alloc(0))
	]);
}

// "any" icons fill the tile; maskable icons keep the artwork inside the central 80% safe zone.
const files = [
	['icon-192.png', 192, 0.22],
	['icon-512.png', 512, 0.22],
	['icon-maskable-192.png', 192, 0.15],
	['icon-maskable-512.png', 512, 0.15],
	['apple-touch-icon.png', 180, 0.2]
];
for (const [name, size, r0] of files) {
	writeFileSync(join(out, name), png(size, r0));
	console.log('wrote', name);
}
