import {
	AdditiveBlending,
	BackSide,
	BufferAttribute,
	BufferGeometry,
	Color,
	ColorManagement,
	DoubleSide,
	Group,
	LineBasicMaterial,
	LineSegments,
	Material,
	Mesh,
	MeshBasicMaterial,
	PerspectiveCamera,
	PlaneGeometry,
	Points,
	RingGeometry,
	Scene,
	ShaderMaterial,
	SphereGeometry,
	WebGLRenderer
} from 'three';
import type { PersonaTheme } from '#lib/api.ts';
import type { OrbState } from '#lib/state/orb.svelte.ts';
import { DEFAULT_THEME, orbPalette, type OrbColors } from '#lib/theme.ts';
import {
	CORE_FRAG,
	CORE_VERT,
	HALO_FRAG,
	HALO_VERT,
	MOTE_FRAG,
	MOTE_VERT,
	SHELL_FRAG,
	SHELL_VERT
} from './shaders';

// Shaders author colours directly in display space; skip three's linear<->sRGB conversion.
ColorManagement.enabled = false;

interface Params {
	energy: number;
	swirl: number;
	disp: number;
	rim: number;
	ripple: number;
	breathe: number;
	halo: number;
	flash: number;
}

/** Target motion per state; the scene eases toward these every frame. Colours come from the persona palette. */
const PRESETS: Record<OrbState, Params> = {
	idle: {
		energy: 1.0, swirl: 0.3, disp: 0.045, rim: 1.0,
		ripple: 0, breathe: 0.035, halo: 0.55, flash: 0
	},
	listening: {
		energy: 1.45, swirl: 0.7, disp: 0.07, rim: 1.35,
		ripple: 1, breathe: 0.05, halo: 0.85, flash: 0
	},
	thinking: {
		energy: 1.25, swirl: 2.1, disp: 0.12, rim: 1.15,
		ripple: 0, breathe: 0.02, halo: 0.7, flash: 0
	},
	speaking: {
		energy: 1.3, swirl: 0.9, disp: 0.08, rim: 1.25,
		ripple: 0, breathe: 0.02, halo: 0.8, flash: 0
	},
	error: {
		energy: 1.1, swirl: 0.5, disp: 0.09, rim: 1.2,
		ripple: 0, breathe: 0.03, halo: 0.75, flash: 1
	}
};

type NumKey = keyof Params;
const NUM_KEYS: NumKey[] = [
	'energy', 'swirl', 'disp', 'rim', 'ripple', 'breathe', 'halo', 'flash'
];

export interface OrbSceneOptions {
	getAmplitude: () => number;
	/** Mobile / weak GPU: lower geometry, fewer motes, capped pixel ratio. */
	lowPower: boolean;
	onContextLost?: () => void;
	onContextRestored?: () => void;
}

export class OrbScene {
	private readonly renderer: WebGLRenderer;
	private readonly scene = new Scene();
	private readonly camera = new PerspectiveCamera(32, 1, 0.1, 60);
	private readonly container: HTMLElement;
	private readonly opts: OrbSceneOptions;

	private readonly core: Mesh;
	private readonly halo: Mesh;
	private readonly ticks: LineSegments;
	private readonly ringGroup = new Group();
	private readonly motes: Points;

	private palette = orbPalette(DEFAULT_THEME);
	private state: OrbState = 'idle';
	private readonly colorA = new Color(this.palette.idle.a);
	private readonly colorB = new Color(this.palette.idle.b);
	private readonly targetA = new Color(this.palette.idle.a);
	private readonly targetB = new Color(this.palette.idle.b);
	private readonly cur: Record<NumKey, number> = { ...numeric(PRESETS.idle) };
	private target: Params = PRESETS.idle;
	private amp = 0;

	private readonly coreU;
	private readonly shellU;
	private readonly haloU;
	private readonly moteU;
	private readonly disposables: { dispose(): void }[] = [];

	private raf = 0;
	private running = false;
	private time = 3;
	private last = 0;
	private reduced: boolean;
	private pixelRatio: number;
	private frames = 0;
	private frameSum = 0;
	private width = 0;
	private height = 0;
	private readonly ro: ResizeObserver;
	private readonly mq = matchMedia('(prefers-reduced-motion: reduce)');
	private readonly canvas: HTMLCanvasElement;

	constructor(container: HTMLElement, opts: OrbSceneOptions) {
		this.container = container;
		this.opts = opts;
		this.reduced = this.mq.matches;
		this.pixelRatio = Math.min(window.devicePixelRatio || 1, opts.lowPower ? 1.5 : 2);

		// Throws when WebGL is unavailable; the caller falls back to a CSS orb.
		this.renderer = new WebGLRenderer({
			antialias: !opts.lowPower,
			alpha: true,
			powerPreference: 'default'
		});
		this.renderer.setClearColor(0x000000, 0);
		this.renderer.setPixelRatio(this.pixelRatio);
		this.canvas = this.renderer.domElement;
		this.canvas.style.cssText = 'display:block;width:100%;height:100%';
		this.canvas.setAttribute('aria-hidden', 'true');
		container.appendChild(this.canvas);

		const low = opts.lowPower;

		// --- core ---
		const coreGeo = new SphereGeometry(1, low ? 48 : 96, low ? 36 : 72);
		this.coreU = {
			uTime: { value: 0 },
			uSwirl: { value: 0.3 },
			uDisp: { value: 0.045 },
			uAmp: { value: 0 },
			uEnergy: { value: 1 },
			uRim: { value: 1 },
			uRipple: { value: 0 },
			uFlash: { value: 0 },
			uColorA: { value: this.colorA },
			uColorB: { value: this.colorB }
		};
		const coreMat = new ShaderMaterial({
			vertexShader: CORE_VERT,
			fragmentShader: CORE_FRAG,
			uniforms: this.coreU,
			transparent: true
		});
		this.core = new Mesh(coreGeo, coreMat);
		this.track(coreGeo, coreMat);

		// --- atmosphere shell ---
		const shellGeo = new SphereGeometry(1.34, 48, 32);
		this.shellU = { uColor: { value: this.colorA }, uIntensity: { value: 0.9 } };
		const shellMat = new ShaderMaterial({
			vertexShader: SHELL_VERT,
			fragmentShader: SHELL_FRAG,
			uniforms: this.shellU,
			side: BackSide,
			transparent: true,
			blending: AdditiveBlending,
			depthWrite: false
		});
		const shell = new Mesh(shellGeo, shellMat);
		shell.renderOrder = -2;
		this.track(shellGeo, shellMat);

		// --- halo (bloom stand-in) ---
		const haloGeo = new PlaneGeometry(1, 1);
		this.haloU = { uColor: { value: this.colorA }, uIntensity: { value: 0.55 }, uTime: { value: 0 } };
		const haloMat = new ShaderMaterial({
			vertexShader: HALO_VERT,
			fragmentShader: HALO_FRAG,
			uniforms: this.haloU,
			transparent: true,
			blending: AdditiveBlending,
			depthWrite: false,
			depthTest: false
		});
		this.halo = new Mesh(haloGeo, haloMat);
		this.halo.renderOrder = -3;
		this.halo.position.z = -1.2;
		this.track(haloGeo, haloMat);

		// --- HUD rings ---
		const ringMat = new MeshBasicMaterial({
			color: this.colorA,
			transparent: true,
			opacity: 0.32,
			blending: AdditiveBlending,
			depthWrite: false,
			side: DoubleSide
		});
		this.disposables.push(ringMat);
		const ringDefs: [number, number, number, number][] = [
			[1.62, 1.632, 1.15, 0.2],
			[1.9, 1.908, 0.9, -0.5]
		];
		for (const [inner, outer, tilt, z] of ringDefs) {
			const geo = new RingGeometry(inner, outer, 160);
			const ring = new Mesh(geo, ringMat);
			ring.rotation.set(tilt, 0, z);
			this.ringGroup.add(ring);
			this.disposables.push(geo);
		}

		// Tick-mark bezel in the screen plane.
		const tickPos: number[] = [];
		const N = 90;
		for (let i = 0; i < N; i++) {
			const a = (i / N) * Math.PI * 2;
			const major = i % 5 === 0;
			const r0 = 2.12;
			const r1 = major ? 2.26 : 2.19;
			tickPos.push(Math.cos(a) * r0, Math.sin(a) * r0, 0, Math.cos(a) * r1, Math.sin(a) * r1, 0);
		}
		const tickGeo = new BufferGeometry();
		tickGeo.setAttribute('position', new BufferAttribute(new Float32Array(tickPos), 3));
		const tickMat = new LineBasicMaterial({
			color: this.colorA,
			transparent: true,
			opacity: 0.4,
			blending: AdditiveBlending,
			depthWrite: false
		});
		this.ticks = new LineSegments(tickGeo, tickMat);
		this.track(tickGeo, tickMat);

		// --- motes ---
		const count = low ? 70 : 240;
		const pos = new Float32Array(count * 3);
		const size = new Float32Array(count);
		const phase = new Float32Array(count);
		for (let i = 0; i < count; i++) {
			const r = 1.45 + Math.random() * 1.15;
			const u = Math.random() * 2 - 1;
			const th = Math.random() * Math.PI * 2;
			const s = Math.sqrt(1 - u * u);
			pos.set([r * s * Math.cos(th), r * u, r * s * Math.sin(th)], i * 3);
			size[i] = 0.6 + Math.random() * 1.6;
			phase[i] = Math.random();
		}
		const moteGeo = new BufferGeometry();
		moteGeo.setAttribute('position', new BufferAttribute(pos, 3));
		moteGeo.setAttribute('aSize', new BufferAttribute(size, 1));
		moteGeo.setAttribute('aPhase', new BufferAttribute(phase, 1));
		this.moteU = {
			uTime: { value: 0 },
			uPx: { value: this.pixelRatio * 10 },
			uColor: { value: this.colorA },
			uIntensity: { value: 0.9 }
		};
		const moteMat = new ShaderMaterial({
			vertexShader: MOTE_VERT,
			fragmentShader: MOTE_FRAG,
			uniforms: this.moteU,
			transparent: true,
			blending: AdditiveBlending,
			depthWrite: false
		});
		this.motes = new Points(moteGeo, moteMat);
		this.motes.renderOrder = 2;
		this.track(moteGeo, moteMat);

		this.ringGroup.renderOrder = 1;
		this.ticks.renderOrder = 1;
		this.scene.add(this.halo, shell, this.core, this.ringGroup, this.ticks, this.motes);

		// --- wiring ---
		this.ro = new ResizeObserver(() => this.resize());
		this.ro.observe(container);
		this.resize();

		document.addEventListener('visibilitychange', this.onVisibility);
		this.mq.addEventListener('change', this.onMotionPref);
		this.canvas.addEventListener('webglcontextlost', this.onLost);
		this.canvas.addEventListener('webglcontextrestored', this.onRestored);
	}

	private track(geo: { dispose(): void }, mat: Material) {
		this.disposables.push(geo, mat);
	}

	setState(state: OrbState): void {
		this.state = state;
		this.target = PRESETS[state];
		this.retarget();
	}

	/** New persona colours: the orb eases to them like it eases between states. */
	setTheme(theme: PersonaTheme): void {
		this.palette = orbPalette(theme);
		this.retarget();
	}

	/** The colours the scene is heading for (tests and debugging). */
	get targetColors(): OrbColors {
		return { a: `#${this.targetA.getHexString()}`, b: `#${this.targetB.getHexString()}` };
	}

	private retarget(): void {
		const c = this.palette[this.state];
		this.targetA.set(c.a);
		this.targetB.set(c.b);
		if (this.reduced) {
			this.snap();
			this.renderOnce();
		}
	}

	start(): void {
		if (this.running || document.hidden) return;
		if (this.reduced) {
			this.renderOnce();
			return;
		}
		this.running = true;
		this.last = performance.now();
		this.raf = requestAnimationFrame(this.frame);
	}

	private stop(): void {
		this.running = false;
		cancelAnimationFrame(this.raf);
	}

	dispose(): void {
		this.stop();
		this.ro.disconnect();
		document.removeEventListener('visibilitychange', this.onVisibility);
		this.mq.removeEventListener('change', this.onMotionPref);
		this.canvas.removeEventListener('webglcontextlost', this.onLost);
		this.canvas.removeEventListener('webglcontextrestored', this.onRestored);
		for (const d of this.disposables) d.dispose();
		this.renderer.dispose();
		this.renderer.forceContextLoss();
		this.canvas.remove();
	}

	// ---- loop ----

	private frame = (now: number): void => {
		if (!this.running) return;
		this.raf = requestAnimationFrame(this.frame);
		const dt = Math.min(0.05, (now - this.last) / 1000);
		this.last = now;
		this.time += dt;
		this.adaptQuality(dt);
		this.update(dt);
		this.renderer.render(this.scene, this.camera);
	};

	private renderOnce(): void {
		this.update(0);
		this.renderer.render(this.scene, this.camera);
	}

	private update(dt: number): void {
		const k = 1 - Math.exp(-dt * 4.5);
		if (this.reduced) this.snap();
		else {
			for (const key of NUM_KEYS) this.cur[key] += (this.target[key] - this.cur[key]) * k;
			this.colorA.lerp(this.targetA, k);
			this.colorB.lerp(this.targetB, k);
		}
		const goal = this.reduced ? 0.25 : this.opts.getAmplitude();
		this.amp += (goal - this.amp) * (1 - Math.exp(-dt * 16));
		if (this.reduced) this.amp = goal;

		const t = this.time;
		const c = this.cur;
		const u = this.coreU;
		u.uTime.value = t;
		u.uSwirl.value = c.swirl;
		u.uDisp.value = c.disp;
		u.uAmp.value = this.amp;
		u.uEnergy.value = c.energy;
		u.uRim.value = c.rim;
		u.uRipple.value = c.ripple;
		u.uFlash.value = c.flash;

		this.haloU.uTime.value = t;
		this.haloU.uIntensity.value = c.halo * (1 + this.amp * 0.8);
		this.moteU.uTime.value = t;
		this.shellU.uIntensity.value = 0.75 * c.energy + this.amp * 0.5;

		// Idle breathing (~5 s period) plus amplitude pulse.
		const s = 1 + c.breathe * Math.sin(t * 1.25) + this.amp * 0.1;
		this.core.scale.setScalar(s);
		this.core.rotation.y = t * 0.06 * (0.5 + c.swirl);

		this.ringGroup.rotation.y = t * 0.07 * (0.6 + c.swirl * 0.5);
		this.ringGroup.rotation.z = Math.sin(t * 0.2) * 0.1;
		this.ticks.rotation.z = -t * 0.04 * (0.6 + c.swirl * 0.4);
		this.motes.rotation.y = t * 0.03 * (0.6 + c.swirl);
		this.motes.rotation.x = Math.sin(t * 0.1) * 0.15;
	}

	private snap(): void {
		for (const key of NUM_KEYS) this.cur[key] = this.target[key];
		this.colorA.copy(this.targetA);
		this.colorB.copy(this.targetB);
	}

	/** If the first seconds run slowly on a high-DPR display, drop to 1x once. */
	private adaptQuality(dt: number): void {
		if (this.pixelRatio <= 1 || this.frames > 150) return;
		this.frames++;
		if (this.frames > 20) this.frameSum += dt;
		if (this.frames === 150 && this.frameSum / 130 > 0.027) {
			this.pixelRatio = 1;
			this.renderer.setPixelRatio(1);
			this.moteU.uPx.value = 10;
			this.resize();
		}
	}

	private resize(): void {
		const w = Math.max(1, this.container.clientWidth);
		const h = Math.max(1, this.container.clientHeight);
		this.width = w;
		this.height = h;
		this.renderer.setSize(w, h, false);
		this.moteU.uPx.value = this.pixelRatio * 10;

		const aspect = w / h;
		this.camera.aspect = aspect;
		const tanHalf = Math.tan((this.camera.fov * Math.PI) / 360);
		const extent = 2.4; // world units that must fit on the shorter axis
		const dist = Math.max(extent / tanHalf, extent / (tanHalf * aspect));
		this.camera.position.set(0, 0, dist);
		this.camera.updateProjectionMatrix();

		// Halo plane sized to the inscribed circle at its own depth so it fades out before the edge.
		const haloDist = dist + 1.2;
		const visH = 2 * haloDist * tanHalf;
		this.halo.scale.setScalar(Math.min(visH, visH * aspect) * 1.02);

		if (!this.running) this.renderOnce();
	}

	// ---- environment ----

	private onVisibility = (): void => {
		if (document.hidden) this.stop();
		else this.start();
	};

	private onMotionPref = (e: MediaQueryListEvent): void => {
		this.reduced = e.matches;
		if (this.reduced) {
			this.stop();
			this.snap();
			this.renderOnce();
		} else this.start();
	};

	private onLost = (e: Event): void => {
		e.preventDefault();
		this.stop();
		this.opts.onContextLost?.();
	};

	private onRestored = (): void => {
		this.opts.onContextRestored?.();
		this.start();
	};

	/** Debug/screenshots only. */
	get size(): [number, number] {
		return [this.width, this.height];
	}
}

function numeric(p: Params): Record<NumKey, number> {
	const out = {} as Record<NumKey, number>;
	for (const k of NUM_KEYS) out[k] = p[k];
	return out;
}
