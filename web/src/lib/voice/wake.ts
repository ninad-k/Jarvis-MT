/**
 * Hands-free routing: what to do with one transcribed speech segment. Pure, so it is unit tested.
 *
 * Rules (SPEC: safety flow is identical for every persona):
 * - Without a confirmation open, a segment only counts when it starts with a wake phrase, or when
 *   the previous segment was a bare wake phrase and this one is the command that follows.
 * - While a confirmation is open, normal commands are ignored. "Cancel" is always honoured (it only
 *   reduces risk). "Confirm" is honoured only when the wake phrase came with it: an open microphone
 *   must never be able to place a trade because a television said a word.
 * - A wake phrase naming a different persona while a confirmation is open is ignored, so the face
 *   never changes mid-flow.
 */
import type { TranscribeResponse, VoiceIntent } from '../api';
import type { PendingState } from './intent';

export type WakeRoute =
	| { kind: 'ignore'; reason: 'no-wake' | 'busy' | 'pending' | 'other-persona' | 'empty' }
	/** Wake phrase with nothing after it: listen for the next segment as the command. */
	| { kind: 'arm'; persona: string | null }
	| { kind: 'run'; persona: string | null; text: string; intent: VoiceIntent };

export interface WakeContext {
	pending: PendingState;
	/** The previous segment was a bare wake phrase and its command is awaited. */
	awaitingCommand: boolean;
	activePersona: string | null;
}

const norm = (s: string) =>
	s
		.toLowerCase()
		.replace(/[^a-z' ]+/g, ' ')
		.replace(/\s+/g, ' ')
		.trim();

const CONFIRM = new Set(['confirm', 'confirm it', 'yes confirm', 'go ahead', 'yes go ahead', 'do it', 'yes do it', 'execute', 'execute it']);
const CANCEL = new Set(['cancel', 'cancel it', 'cancel that', 'never mind', 'nevermind', 'abort', 'stop', 'no', 'dont', "don't", 'no cancel']);

/** Exact-phrase check of the words after the wake phrase. Anything else is "other". */
export function classifyCommand(command: string): VoiceIntent {
	const t = norm(command);
	if (CONFIRM.has(t)) return 'confirm';
	if (CANCEL.has(t)) return 'cancel';
	return 'other';
}

export function routeWake(res: TranscribeResponse, ctx: WakeContext): WakeRoute {
	const wake = res.wake;
	const detected = wake?.detected === true;
	const personaId = detected ? (wake?.persona ?? null) : null;
	const command = detected ? (wake?.command ?? '').trim() : '';
	const { pending } = ctx;

	if (pending.open) {
		if (pending.busy) return { kind: 'ignore', reason: 'busy' };
		if (detected && personaId && ctx.activePersona && personaId !== ctx.activePersona)
			return { kind: 'ignore', reason: 'other-persona' };
		const text = detected ? command : res.text.trim();
		const intent = classifyCommand(text);
		// Server intent is for the whole utterance; it can only ever add a cancel.
		if (intent === 'cancel' || (!detected && res.intent === 'cancel')) return { kind: 'run', persona: personaId, text: text || res.text.trim(), intent: 'cancel' };
		if (intent === 'confirm' && detected) return { kind: 'run', persona: personaId, text, intent: 'confirm' };
		return { kind: 'ignore', reason: 'pending' };
	}

	if (detected) {
		if (command.length === 0) return { kind: 'arm', persona: personaId };
		// With nothing to confirm, a spoken "confirm" is just words for the assistant.
		const intent = classifyCommand(command);
		return { kind: 'run', persona: personaId, text: command, intent: intent === 'confirm' ? 'other' : intent };
	}
	if (ctx.awaitingCommand) {
		const text = res.text.trim();
		if (text.length === 0) return { kind: 'ignore', reason: 'empty' };
		return { kind: 'run', persona: null, text, intent: 'other' };
	}
	return { kind: 'ignore', reason: 'no-wake' };
}
