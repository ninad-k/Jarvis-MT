import type { TranscribeResponse } from '../api';

/** What the UI knows about the confirmation modal when speech arrives. */
export interface PendingState {
	/** A preview is on screen and waiting for a decision. */
	open: boolean;
	/** Its countdown has run out. */
	expired: boolean;
	/** A confirm request is already in flight or results are showing. */
	busy: boolean;
}

export type VoiceRoute =
	| { kind: 'confirm' }
	| { kind: 'cancel' }
	/** A spoken "confirm" arrived but the preview is no longer confirmable. */
	| { kind: 'expired' }
	| { kind: 'chat'; text: string }
	| { kind: 'empty' };

/**
 * Decide what a transcript means. The server classifies the intent; the UI only acts on
 * confirm/cancel while a preview is actually pending, otherwise the words go to chat.
 */
export function routeTranscript(
	res: Pick<TranscribeResponse, 'text' | 'intent'>,
	pending: PendingState
): VoiceRoute {
	const text = res.text.trim();
	if (text.length === 0) return { kind: 'empty' };
	if (pending.open && (res.intent === 'confirm' || res.intent === 'cancel')) {
		if (pending.busy) return { kind: 'empty' };
		if (res.intent === 'cancel') return { kind: 'cancel' };
		return pending.expired ? { kind: 'expired' } : { kind: 'confirm' };
	}
	return { kind: 'chat', text };
}
