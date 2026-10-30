// Draws the 1024px orb icon (assets/icon-source.png) that `tauri icon` expands
// into every platform size. Run: pnpm -C apps/desktop icons
import { mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import sharp from 'sharp';

const out = resolve(dirname(fileURLToPath(import.meta.url)), '../assets/icon-source.png');

const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">
  <defs>
    <radialGradient id="bg" cx="50%" cy="38%" r="75%">
      <stop offset="0" stop-color="#12234a"/>
      <stop offset="1" stop-color="#050912"/>
    </radialGradient>
    <radialGradient id="orb" cx="38%" cy="32%" r="70%">
      <stop offset="0" stop-color="#e3f9ff"/>
      <stop offset="0.28" stop-color="#4cc9f0"/>
      <stop offset="0.72" stop-color="#1d4ed8"/>
      <stop offset="1" stop-color="#0b1b4a"/>
    </radialGradient>
    <radialGradient id="glow" cx="50%" cy="50%" r="50%">
      <stop offset="0.55" stop-color="#4cc9f0" stop-opacity="0.55"/>
      <stop offset="1" stop-color="#4cc9f0" stop-opacity="0"/>
    </radialGradient>
  </defs>
  <rect width="1024" height="1024" rx="224" fill="url(#bg)"/>
  <circle cx="512" cy="512" r="400" fill="url(#glow)"/>
  <circle cx="512" cy="512" r="262" fill="url(#orb)"/>
  <ellipse cx="512" cy="512" rx="372" ry="120" fill="none" stroke="#4cc9f0" stroke-opacity="0.75"
           stroke-width="14" transform="rotate(-24 512 512)"/>
  <ellipse cx="452" cy="420" rx="86" ry="52" fill="#ffffff" fill-opacity="0.35" transform="rotate(-30 452 420)"/>
</svg>`;

mkdirSync(dirname(out), { recursive: true });
await sharp(Buffer.from(svg)).png().toFile(out);
console.log('wrote', out);
