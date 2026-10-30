import { describe, expect, it } from 'vitest';
import {
	DEFAULT_THEME,
	MIN_TEXT_CONTRAST,
	SURFACE,
	applyTheme,
	contrast,
	darken,
	ensureContrast,
	hexToRgb,
	lighten,
	mix,
	orbPalette,
	rgbToHex,
	sanitizeTheme,
	themeVars
} from './theme';

// The two shipped persona packs (personas/jarvis.toml, personas/friday.toml).
const JARVIS = { accent: '#38bdf8', accent_soft: '#0c4a6e', orb_core: '#7dd3fc', orb_glow: '#0ea5e9' };
const FRIDAY = { accent: '#f5b942', accent_soft: '#5c3d0a', orb_core: '#fcd77f', orb_glow: '#f59e0b' };

/** Every surface text sits on: from the page background to the lightest raised panel. */
const SURFACES = ['#04070d', '#070d17', '#0a1320', '#102033'];

describe('colour maths', () => {
	it('round-trips hex and rejects anything else', () => {
		expect(hexToRgb('#38bdf8')).toEqual([56, 189, 248]);
		expect(rgbToHex([56, 189, 248])).toBe('#38bdf8');
		expect(() => hexToRgb('red')).toThrow(RangeError);
		expect(() => hexToRgb('#fff')).toThrow(RangeError);
	});

	it('mixes, lightens and darkens', () => {
		expect(mix('#000000', '#ffffff', 0.5)).toBe('#808080');
		expect(mix('#102030', '#102030', 0.7)).toBe('#102030');
		expect(lighten('#000000', 1)).toBe('#ffffff');
		expect(darken('#ffffff', 1)).toBe('#000000');
		expect(mix('#000000', '#ffffff', 9)).toBe('#ffffff');
	});

	it('computes WCAG contrast', () => {
		expect(contrast('#000000', '#ffffff')).toBeCloseTo(21, 0);
		expect(contrast('#777777', '#777777')).toBeCloseTo(1, 5);
		// argument order does not matter
		expect(contrast('#ffffff', '#04070d')).toBeCloseTo(contrast('#04070d', '#ffffff'), 5);
	});
});

describe('persona themes stay readable (WCAG AA)', () => {
	for (const [name, theme] of Object.entries({ Jarvis: JARVIS, Friday: FRIDAY })) {
		it(`${name}: the accent reads as text on every HUD surface`, () => {
			const accent = themeVars(theme)['--persona-accent'];
			for (const bg of SURFACES) {
				expect(contrast(accent, bg), `${accent} on ${bg}`).toBeGreaterThanOrEqual(MIN_TEXT_CONTRAST);
			}
		});

		it(`${name}: accent text on its own 15 % and 25 % tint (active buttons) is still readable`, () => {
			const accent = themeVars(theme)['--persona-accent'];
			for (const t of [0.15, 0.25]) {
				expect(contrast(accent, mix(SURFACE, accent, t))).toBeGreaterThanOrEqual(MIN_TEXT_CONTRAST);
			}
		});
	}

	it('gold on the dark background is far above the minimum', () => {
		expect(contrast('#f5b942', '#04070d')).toBeGreaterThan(9);
	});

	it('lifts a too-dark accent until it passes, and leaves a good one alone', () => {
		const dark = '#1d4ed8';
		expect(contrast(dark, SURFACE)).toBeLessThan(MIN_TEXT_CONTRAST);
		const fixed = ensureContrast(dark);
		expect(contrast(fixed, SURFACE)).toBeGreaterThanOrEqual(MIN_TEXT_CONTRAST);
		expect(ensureContrast('#f5b942')).toBe('#f5b942');
	});

	it('red state colours keep their contrast on the HUD (LIVE banner, error text)', () => {
		// bg-down with text-void (the LIVE banner) and text-down on the panels
		expect(contrast('#04070d', '#ff5d73')).toBeGreaterThanOrEqual(MIN_TEXT_CONTRAST);
		expect(contrast('#ff5d73', '#0a1320')).toBeGreaterThanOrEqual(MIN_TEXT_CONTRAST);
	});
});

describe('sanitizeTheme', () => {
	it('keeps valid colours, lower-cased', () => {
		expect(sanitizeTheme({ ...JARVIS, accent: '#38BDF8' })).toEqual(JARVIS);
	});

	it('replaces anything that is not #rrggbb with the default for that slot', () => {
		const bad = sanitizeTheme({
			accent: 'red',
			accent_soft: '#12',
			orb_core: 'url(javascript:alert(1))',
			orb_glow: '#0ea5e9; background: url(x)'
		});
		expect(bad).toEqual({ ...DEFAULT_THEME, orb_glow: DEFAULT_THEME.orb_glow });
	});

	it('copes with a missing or non-string theme', () => {
		expect(sanitizeTheme(null)).toEqual(DEFAULT_THEME);
		expect(sanitizeTheme(undefined)).toEqual(DEFAULT_THEME);
		expect(sanitizeTheme({ accent: 42 as unknown as string })).toEqual(DEFAULT_THEME);
	});
});

describe('applyTheme', () => {
	function fakeEl() {
		const props = new Map<string, string>();
		const dataset: Record<string, string> = {};
		return {
			props,
			el: { style: { setProperty: (k: string, v: string) => void props.set(k, v) }, dataset: dataset as unknown as DOMStringMap }
		};
	}

	it('writes the four persona custom properties', () => {
		const { el, props } = fakeEl();
		applyTheme(el, FRIDAY, 'friday');
		expect([...props.keys()].sort()).toEqual([
			'--persona-accent',
			'--persona-accent-soft',
			'--persona-orb-core',
			'--persona-orb-glow'
		]);
		expect(props.get('--persona-accent')).toBe('#f5b942');
		expect(props.get('--persona-accent-soft')).toBe('#5c3d0a');
		expect(props.get('--persona-orb-core')).toBe('#fcd77f');
		expect(props.get('--persona-orb-glow')).toBe('#f59e0b');
		expect(el.dataset?.persona).toBe('friday');
	});

	it('switching persona replaces every value', () => {
		const { el, props } = fakeEl();
		applyTheme(el, JARVIS, 'jarvis');
		applyTheme(el, FRIDAY, 'friday');
		expect(props.get('--persona-accent')).toBe('#f5b942');
		expect(props.get('--persona-orb-glow')).toBe('#f59e0b');
	});

	it('never writes an unvalidated value into the page', () => {
		const { el, props } = fakeEl();
		applyTheme(el, { ...JARVIS, accent: 'red; --x: 1' });
		expect(props.get('--persona-accent')).toBe(ensureContrast(DEFAULT_THEME.accent));
		for (const v of props.values()) expect(v).toMatch(/^#[0-9a-f]{6}$/);
	});
});

describe('orbPalette', () => {
	it('idle, listening and speaking follow the persona', () => {
		const j = orbPalette(JARVIS);
		const f = orbPalette(FRIDAY);
		for (const state of ['idle', 'listening', 'speaking'] as const) {
			expect(j[state].a, state).not.toBe(f[state].a);
			expect(j[state].b, state).not.toBe(f[state].b);
		}
		expect(j.idle.a).toBe(JARVIS.orb_core);
		expect(f.idle.a).toBe(FRIDAY.orb_core);
	});

	it('error is the same red for every persona', () => {
		expect(orbPalette(JARVIS).error).toEqual(orbPalette(FRIDAY).error);
		expect(orbPalette(FRIDAY).error.a).toBe('#ff4d5e');
	});

	it('thinking keeps its violet and amber identity but leans toward the persona', () => {
		const j = orbPalette(JARVIS).thinking;
		const f = orbPalette(FRIDAY).thinking;
		expect(j.a).not.toBe(f.a);
		// still violet-ish: blue channel dominates red/green for the bright colour
		const [r, g, b] = hexToRgb(j.a);
		expect(b).toBeGreaterThan(g);
		expect(b).toBeGreaterThan(r - 20);
		// and still clearly different from idle
		expect(j.a).not.toBe(orbPalette(JARVIS).idle.a);
	});

	it('only produces plain hex colours', () => {
		for (const theme of [JARVIS, FRIDAY, DEFAULT_THEME]) {
			for (const c of Object.values(orbPalette(theme))) {
				expect(c.a).toMatch(/^#[0-9a-f]{6}$/);
				expect(c.b).toMatch(/^#[0-9a-f]{6}$/);
			}
		}
	});
});
