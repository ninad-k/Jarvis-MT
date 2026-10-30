import { status } from './hud.svelte';

const KEY = 'jarvis.provider';

function read(): string | null {
	try {
		return localStorage.getItem(KEY);
	} catch {
		return null;
	}
}

function write(name: string): void {
	try {
		localStorage.setItem(KEY, name);
	} catch {
		/* storage unavailable: the selection just won't persist */
	}
}

class ProviderStore {
	private chosen = $state<string | null>(read());

	get providers() {
		return status.data?.llm.providers ?? [];
	}

	/** The saved choice if it still exists and is usable, else the server default. */
	get selected(): string | null {
		const llm = status.data?.llm;
		if (!llm) return null;
		const saved = llm.providers.find((p) => p.name === this.chosen && p.available);
		return saved?.name ?? llm.default;
	}

	select(name: string): void {
		this.chosen = name;
		write(name);
	}
}

export const provider = new ProviderStore();
