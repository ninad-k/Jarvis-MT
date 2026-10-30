/**
 * Persona theming: validation of the colours a persona pack supplies, the derived palette for the
 * orb, WCAG contrast helpers and `applyTheme`, which writes the CSS custom properties.
 * Pure functions only (no DOM except through the element passed to `applyTheme`), so it is unit tested.
 */
import type { PersonaTheme } from './api';

/** Jarvis colours (personas/jarvis.toml): the look before the server has answered. */
export const DEFAULT_THEME: PersonaTheme = {
	accent: '#38bdf8',
	accent_soft: '#0c4a6e',
	orb_core: '#7dd3fc',
	orb_glow: '#0ea5e9'
};

export const DEFAULT_PERSONA_NAME = 'Jarvis';

/** The lightest HUD surface accent text sits on (`--color-raise`); darker panels only help. */
export const SURFACE = '#102033';

/** Text contrast the UI promises for accent-coloured text (WCAG 2.1 AA, normal text). */
export const MIN_TEXT_CONTRAST = 4.5;

const HEX = /^#[0-9a-fA-F]{6}$/;

export type Rgb = readonly [number, number, number];

export function isHex(v: unknown): v is string {
	return typeof v === 'string' && HEX.test(v);
}

export function hexToRgb(hex: string): Rgb {
	if (!isHex(hex)) throw new RangeError(`not a #rrggbb colour: ${String(hex)}`);
	return [
		parseInt(hex.slice(1, 3), 16),
		parseInt(hex.slice(3, 5), 16),
		parseInt(hex.slice(5, 7), 16)
	];
}

const byte = (n: number) => Math.max(0, Math.min(255, Math.round(n)));

export function rgbToHex([r, g, b]: Rgb): string {
	return `#${[r, g, b].map((c) => byte(c).toString(16).padStart(2, '0')).join('')}`;
}

/** Linear blend: t = 0 gives `a`, t = 1 gives `b`. */
export function mix(a: string, b: string, t: number): string {
	const x = hexToRgb(a);
	const y = hexToRgb(b);
	const k = Math.max(0, Math.min(1, t));
	return rgbToHex([x[0] + (y[0] - x[0]) * k, x[1] + (y[1] - x[1]) * k, x[2] + (y[2] - x[2]) * k]);
}

export const lighten = (hex: string, t: number) => mix(hex, '#ffffff', t);
export const darken = (hex: string, t: number) => mix(hex, '#000000', t);

/** WCAG relative luminance. */
export function luminance(hex: string): number {
	const lin = hexToRgb(hex).map((c) => {
		const s = c / 255;
		return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
	});
	return 0.2126 * lin[0] + 0.7152 * lin[1] + 0.0722 * lin[2];
}

/** WCAG contrast ratio, 1..21. */
export function contrast(a: string, b: string): number {
	const [hi, lo] = [luminance(a), luminance(b)].sort((p, q) => q - p);
	return (hi + 0.05) / (lo + 0.05);
}

/** Lighten `hex` until it reads at `min` contrast on `bg` (unchanged when it already does). */
export function ensureContrast(hex: string, bg: string = SURFACE, min: number = MIN_TEXT_CONTRAST): string {
	let out = hex;
	for (let i = 1; i <= 20 && contrast(out, bg) < min; i++) out = lighten(hex, i / 20);
	return out;
}

/**
 * A theme from server data. Anything that is not a plain `#rrggbb` colour falls back to the
 * default for that slot, so a bad pack can neither break the page nor inject CSS.
 */
export function sanitizeTheme(raw: Partial<Record<keyof PersonaTheme, unknown>> | null | undefined): PersonaTheme {
	const pick = (k: keyof PersonaTheme) => (isHex(raw?.[k]) ? (raw![k] as string).toLowerCase() : DEFAULT_THEME[k]);
	return {
		accent: pick('accent'),
		accent_soft: pick('accent_soft'),
		orb_core: pick('orb_core'),
		orb_glow: pick('orb_glow')
	};
}

/** The custom properties a theme sets, after making the accent readable on the dark surface. */
export function themeVars(theme: PersonaTheme): Record<string, string> {
	const t = sanitizeTheme(theme);
	return {
		'--persona-accent': ensureContrast(t.accent),
		'--persona-accent-soft': t.accent_soft,
		'--persona-orb-core': t.orb_core,
		'--persona-orb-glow': t.orb_glow
	};
}

/** The slice of an element `applyTheme` needs; `document.documentElement` satisfies it. */
export interface StyleTarget {
	style: { setProperty(name: string, value: string): void };
	dataset?: DOMStringMap;
}

export function applyTheme(el: StyleTarget, theme: PersonaTheme, personaId?: string): void {
	for (const [k, v] of Object.entries(themeVars(theme))) el.style.setProperty(k, v);
	if (personaId && el.dataset) el.dataset.persona = personaId;
}

// ---- Orb ---------------------------------------------------------------------

export type OrbMood = 'idle' | 'listening' | 'thinking' | 'speaking' | 'error';
export interface OrbColors {
	/** Bright colour: rim, veins, halo, rings. */
	a: string;
	/** Deep colour: the body of the sphere. */
	b: string;
}

/**
 * Orb colours per state. Idle, listening and speaking follow the persona. Thinking keeps its
 * violet/amber identity but leans 25 % toward the persona so it never looks like another app.
 * Error stays pure red for every persona: it has to be unmistakable.
 */
export function orbPalette(theme: PersonaTheme): Record<OrbMood, OrbColors> {
	const t = sanitizeTheme(theme);
	return {
		idle: { a: t.orb_core, b: darken(t.orb_glow, 0.5) },
		listening: { a: lighten(t.orb_core, 0.25), b: darken(t.orb_glow, 0.3) },
		thinking: { a: mix('#9b7bff', t.orb_core, 0.25), b: mix('#f59e0b', t.orb_glow, 0.25) },
		speaking: { a: lighten(t.orb_core, 0.1), b: mix(t.orb_glow, '#12b8a0', 0.3) },
		error: { a: '#ff4d5e', b: '#6b0f1e' }
	};
}
