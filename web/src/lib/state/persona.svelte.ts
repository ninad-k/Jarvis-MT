import { api, type Persona } from '#lib/api.ts';
import { describeError, toApiError } from '#lib/errors.ts';
import { DEFAULT_PERSONA_NAME, DEFAULT_THEME, sanitizeTheme } from '#lib/theme.ts';
import { status } from './hud.svelte';
import { preview } from './preview.svelte';
import { settings } from './settings.svelte';
import { toasts } from './toasts.svelte';

const CACHE_KEY = 'arcmt.persona';

interface Cached {
	id: string;
	name: string;
	theme: Persona['theme'];
}

/** The last persona shown, so a reload paints the right colours before the server has answered. */
function readCache(): Cached | null {
	try {
		const raw = localStorage.getItem(CACHE_KEY);
		if (!raw) return null;
		const v = JSON.parse(raw) as Partial<Cached>;
		if (typeof v.id !== 'string' || typeof v.name !== 'string') return null;
		return { id: v.id, name: v.name.slice(0, 40), theme: sanitizeTheme(v.theme) };
	} catch {
		return null;
	}
}

function writeCache(p: Persona): void {
	try {
		localStorage.setItem(CACHE_KEY, JSON.stringify({ id: p.id, name: p.name, theme: p.theme }));
	} catch {
		/* storage unavailable: the next load just starts with the default look */
	}
}

/**
 * Personas: the list from `GET /api/personas` and the active one (the user's setting). The active
 * persona drives the name used in the UI and the theme (see `applyTheme` in theme.ts).
 * The safety flow is the same for every persona, so the only rule here is that the face does not
 * change while a confirmation is open.
 */
class PersonaStore {
	list = $state<Persona[]>([]);
	/** The server's default persona. */
	defaultId = $state<string | null>(null);
	loaded = $state(false);
	/** `active` from GET /api/personas: the fallback when settings are unavailable. */
	private listActive = $state<string | null>(null);
	/** A switch is in flight. */
	switching = $state(false);
	/** Id shown while a switch is in flight (optimistic) before the server confirms. */
	private pendingId = $state<string | null>(null);
	private cached = $state<Cached | null>(readCache());
	private inflight: Promise<void> | null = null;

	/** Whether the server has personas at all (GET /api/personas worked). */
	get supported(): boolean {
		return this.list.length > 0;
	}

	get activeId(): string | null {
		return (
			this.pendingId ??
			settings.data?.persona ??
			status.data?.persona?.id ??
			this.listActive ??
			this.cached?.id ??
			this.defaultId
		);
	}

	get current(): Persona | null {
		const id = this.activeId;
		return this.list.find((p) => p.id === id) ?? null;
	}

	get name(): string {
		return this.current?.name ?? status.data?.persona?.name ?? this.cached?.name ?? DEFAULT_PERSONA_NAME;
	}

	/** Display name for a persona id (a chat reply names the persona that answered). */
	nameFor(id: string | undefined): string {
		return (id ? this.list.find((p) => p.id === id)?.name : null) ?? this.name;
	}

	get theme() {
		return this.current?.theme ?? (this.cached ? this.cached.theme : DEFAULT_THEME);
	}

	get wakeWord(): string {
		return this.current?.wake_word ?? `hey ${this.name.toLowerCase()}`;
	}

	/**
	 * Why switching is not allowed, or null. Only an open confirmation locks the controls: a switch
	 * in flight is guarded inside `select` instead, so the button the user just pressed keeps focus.
	 */
	get lockReason(): string | null {
		return preview.isOpen ? 'Finish or cancel the open confirmation before switching' : null;
	}

	/** Load the personas and the user's settings. Idempotent; safe from several pages. */
	ensureLoaded(): Promise<void> {
		if (this.loaded) return Promise.resolve();
		this.inflight ??= (async () => {
			await Promise.all([settings.ensureLoaded(), this.loadList()]);
			this.cache();
		})().finally(() => (this.inflight = null));
		return this.inflight;
	}

	private async loadList(): Promise<void> {
		try {
			const res = await api.personas();
			this.list = res.personas.map((p) => ({ ...p, theme: sanitizeTheme(p.theme) }));
			this.defaultId = res.default;
			this.loaded = true;
			this.listActive = res.active;
		} catch {
			/* older server: no personas; the toggle stays hidden and the default look applies */
		}
	}

	/** Remember the active persona for the next cold start. */
	cache(): void {
		const p = this.current;
		if (p) writeCache(p);
	}

	/** Switch persona: shows at once, rolls back with a message if the server refuses. */
	async select(id: string): Promise<boolean> {
		if (this.lockReason !== null || this.switching || id === this.activeId) return false;
		if (!this.list.some((p) => p.id === id)) return false;
		this.switching = true;
		this.pendingId = id;
		try {
			await settings.update({ persona: id }, { optimistic: true });
			this.pendingId = null;
			this.cache();
			return true;
		} catch (e) {
			this.pendingId = null;
			toasts.error(`Could not switch persona. ${describeError(toApiError(e))}`);
			return false;
		} finally {
			this.switching = false;
		}
	}

	/** The server switched persona itself (a wake phrase named the other one). */
	adoptFromServer(id: string): void {
		if (!this.list.some((p) => p.id === id)) {
			void settings.refresh();
			return;
		}
		settings.adopt({ persona: id });
		this.cache();
	}

	reset(): void {
		this.list = [];
		this.loaded = false;
		this.defaultId = null;
		this.listActive = null;
		this.pendingId = null;
		settings.reset();
	}
}

export const persona = new PersonaStore();
