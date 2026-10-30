import { api, ApiError, type ToolCall } from '#lib/api.ts';
import { orb } from './orb.svelte';
import { provider } from './provider.svelte';

export const MAX_MESSAGE = 2000;

export type ChatMessage =
	| { id: number; role: 'user'; text: string; at: number }
	| {
			id: number;
			role: 'assistant';
			text: string;
			at: number;
			provider: string;
			toolCalls: ToolCall[];
	  }
	| { id: number; role: 'error'; text: string; at: number; code: string };

class ChatStore {
	messages = $state<ChatMessage[]>([]);
	sessionId = $state<string | null>(null);
	pending = $state(false);

	private nextId = 1;

	async send(raw: string): Promise<boolean> {
		const message = raw.trim();
		if (this.pending || message.length === 0 || message.length > MAX_MESSAGE) return false;

		this.messages.push({ id: this.nextId++, role: 'user', text: message, at: Date.now() });
		this.pending = true;
		orb.set('thinking');

		try {
			const res = await api.chat({
				message,
				...(this.sessionId ? { session_id: this.sessionId } : {}),
				...(provider.selected ? { provider: provider.selected } : {})
			});
			this.sessionId = res.session_id;
			this.messages.push({
				id: this.nextId++,
				role: 'assistant',
				text: res.reply,
				at: Date.now(),
				provider: res.provider,
				toolCalls: res.tool_calls ?? []
			});
			orb.speak(1500);
		} catch (e) {
			const err = e instanceof ApiError ? e : new ApiError('unknown', 'Unexpected error');
			this.messages.push({
				id: this.nextId++,
				role: 'error',
				text: err.isOffline
					? 'The Jarvis server is offline. Your message was not sent.'
					: err.message,
				at: Date.now(),
				code: err.code
			});
			orb.flashError();
		} finally {
			this.pending = false;
		}
		return true;
	}

	/** Start a fresh session (clears the transcript; the server drops history on its own). */
	reset(): void {
		if (this.pending) return;
		this.messages = [];
		this.sessionId = null;
	}
}

export const chat = new ChatStore();
