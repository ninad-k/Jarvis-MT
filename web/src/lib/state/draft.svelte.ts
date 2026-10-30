import { untrack } from 'svelte';

const clone = <T>(v: T): T => structuredClone($state.snapshot(v)) as T;
const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

/**
 * An editable copy of a server value. `value` is what the form edits; `dirty` says it differs from
 * the server's. While the form is clean it follows the server (another device saved something);
 * while the user has edits, those are kept. Create it during component setup.
 */
export class Draft<T> {
	value = $state() as T;
	private base = $state() as T;

	constructor(source: () => T) {
		const first = source();
		this.value = clone(first);
		this.base = clone(first);
		$effect(() => {
			const server = clone(source());
			untrack(() => {
				if (same(this.value, this.base)) this.value = clone(server);
				this.base = server;
			});
		});
	}

	get dirty(): boolean {
		return !same(this.value, this.base);
	}

	/** Throw the edits away. */
	revert(): void {
		this.value = clone(this.base);
	}

	/** After a save: show exactly what the server now holds (it may have normalised the input). */
	adopt(server: T): void {
		this.base = clone(server);
		this.value = clone(server);
	}
}

/** Which settings sections have unsaved edits; the page asks before leaving while any do. */
export class DirtyRegistry {
	private map = $state<Record<string, boolean>>({});

	set(id: string, dirty: boolean): void {
		if (this.map[id] !== dirty) this.map = { ...this.map, [id]: dirty };
	}

	clear(id: string): void {
		if (id in this.map) {
			const { [id]: _drop, ...rest } = this.map;
			this.map = rest;
		}
	}

	get any(): boolean {
		return Object.values(this.map).some(Boolean);
	}

	get sections(): string[] {
		return Object.entries(this.map)
			.filter(([, d]) => d)
			.map(([id]) => id);
	}
}
