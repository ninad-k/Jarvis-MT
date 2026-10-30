import { describeError, toApiError } from '#lib/errors.ts';
import { status } from './hud.svelte';
import { settings } from './settings.svelte';
import { toasts } from './toasts.svelte';

const KEY = 'jarvis.provider';

function read(): string | null {
	try {
		return localStorage.getItem(KEY);
	} catch {
		return null;
	}
}

function write(name: string | null): void {
	try {
		if (name === null) localStorage.removeItem(KEY);
		else localStorage.setItem(KEY, name);
	} catch {
		/* storage unavailable: the selection just won't persist */
	}
}

/**
 * The language model for new messages. The choice lives in the user's settings on the server
 * (`llm_provider`, null = server default); a server without settings falls back to this browser.
 */
class ProviderStore {
	private local = $state<string | null>(read());

	get providers() {
		return status.data?.llm.providers ?? [];
	}

	/** The explicit choice, or null for "server default". */
	get explicit(): string | null {
		return settings.data ? settings.data.llm_provider : this.local;
	}

	/** What is in use: the choice if it still exists and is usable, else the server default. */
	get selected(): string | null {
		const llm = status.data?.llm;
		if (!llm) return null;
		const saved = llm.providers.find((p) => p.name === this.explicit && p.available);
		return saved?.name ?? llm.default;
	}

	/** Pass null for the server default. Rolls back with a message if the server refuses. */
	async select(name: string | null): Promise<boolean> {
		if (!settings.data) {
			this.local = name;
			write(name);
			return true;
		}
		try {
			await settings.update({ llm_provider: name }, { optimistic: true });
			return true;
		} catch (e) {
			toasts.error(`Could not change the model. ${describeError(toApiError(e))}`);
			return false;
		}
	}
}

export const provider = new ProviderStore();
