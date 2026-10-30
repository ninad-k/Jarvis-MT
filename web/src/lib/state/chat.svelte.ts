import { api, type ChatResponse, type ToolCall } from '#lib/api.ts';
import { describeError, toApiError } from '#lib/errors.ts';
import { orb } from './orb.svelte';
import { persona } from './persona.svelte';
import { provider } from './provider.svelte';

export const MAX_MESSAGE = 2000;

export type ChatMessage =
	| { id: number; role: 'user'; text: string; at: number; voice: boolean }
	| {
			id: number;
			role: 'assistant';
			text: string;
			at: number;
			provider: string;
			toolCalls: ToolCall[];
			/** Display name of the persona that answered, kept so the label survives a later switch. */
			persona: string;
	  }
	| { id: number; role: 'error'; text: string; at: number; code: string }
	/** A line from the app itself: trade results, cancellations. */
	| { id: number; role: 'note'; text: string; at: number; tone: 'ok' | 'warn' | 'bad' };

class ChatStore {
	messages = $state<ChatMessage[]>([]);
	sessionId = $state<string | null>(null);
	pending = $state(false);

	private nextId = 1;

	/** Add what the user said or typed to the transcript without sending a chat request. */
	addUser(text: string, voice = false): void {
		this.messages.push({ id: this.nextId++, role: 'user', text, at: Date.now(), voice });
	}

	/** Add a line written by the app itself (trade results, cancellations) to the transcript. */
	note(text: string, tone: 'ok' | 'warn' | 'bad' = 'ok'): void {
		this.messages.push({ id: this.nextId++, role: 'note', text, at: Date.now(), tone });
	}

	/**
	 * Send one turn to the chat endpoint. Returns the response, or null when nothing was sent or the
	 * call failed (the failure is already in the transcript). Previews and speech are handled by
	 * the caller (see `turn.svelte.ts`).
	 */
	async send(raw: string, opts: { voice?: boolean; alreadyShown?: boolean } = {}): Promise<ChatResponse | null> {
		const message = raw.trim();
		if (this.pending || message.length === 0 || message.length > MAX_MESSAGE) return null;

		if (!opts.alreadyShown) this.addUser(message, opts.voice === true);
		this.pending = true;
		orb.set('thinking');

		try {
			const res = await api.chat({
				message,
				...(this.sessionId ? { session_id: this.sessionId } : {}),
				...(provider.explicit ? { provider: provider.selected ?? provider.explicit } : {}),
				...(opts.voice ? { voice: true } : {})
			});
			this.sessionId = res.session_id;
			this.messages.push({
				id: this.nextId++,
				role: 'assistant',
				text: res.reply,
				at: Date.now(),
				provider: res.provider,
				persona: persona.nameFor(res.persona),
				toolCalls: res.tool_calls ?? []
			});
			orb.set('idle');
			return res;
		} catch (e) {
			const err = toApiError(e);
			this.messages.push({
				id: this.nextId++,
				role: 'error',
				text: err.isOffline
					? 'The Arc MT server is offline. Your message was not sent.'
					: describeError(err),
				at: Date.now(),
				code: err.code
			});
			orb.flashError();
			return null;
		} finally {
			this.pending = false;
		}
	}

	/** Start a fresh session (clears the transcript; the server drops history on its own). */
	reset(): void {
		if (this.pending) return;
		this.messages = [];
		this.sessionId = null;
	}
}

export const chat = new ChatStore();
