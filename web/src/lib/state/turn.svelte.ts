import type { ChatResponse } from '#lib/api.ts';
import { chat } from './chat.svelte';
import { preview } from './preview.svelte';
import { speech } from './speech.svelte';

/**
 * One conversational turn: send the text, show a preview if the reply proposed a trade, and
 * read the reply aloud when the turn was spoken (or "Read replies aloud" is on).
 * This is the only place a chat reply turns into a confirmation modal.
 */
export async function sendTurn(
	text: string,
	opts: { voice?: boolean; alreadyShown?: boolean } = {}
): Promise<ChatResponse | null> {
	const res = await chat.send(text, opts);
	if (!res) return null;

	if (res.preview) preview.open(res.preview);

	const spoken = (opts.voice === true || speech.readAloud) && speech.available;
	if (spoken) {
		// For a proposed trade, read exactly what will be executed, then start the window.
		const line = res.preview
			? `${res.preview.description}. Say confirm, or cancel.`
			: res.reply;
		void speech.speak(line).finally(() => void preview.readbackDone());
	} else if (res.preview?.timing === 'after_readback') {
		// Nothing will be read aloud, so the 15 s window must start now.
		void preview.readbackDone();
	}
	return res;
}
