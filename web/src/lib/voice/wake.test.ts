import { describe, expect, it } from 'vitest';
import type { TranscribeResponse } from '../api';
import type { PendingState } from './intent';
import { classifyCommand, routeWake, type WakeContext } from './wake';

const none: PendingState = { open: false, expired: false, busy: false };
const open: PendingState = { open: true, expired: false, busy: false };

const ctx = (over: Partial<WakeContext> = {}): WakeContext => ({
	pending: none,
	awaitingCommand: false,
	activePersona: 'jarvis',
	...over
});

function heard(
	text: string,
	wake?: { persona: string; command: string } | null,
	intent: TranscribeResponse['intent'] = 'other'
): TranscribeResponse {
	return {
		text,
		raw: text,
		audio_ms: 1500,
		elapsed_ms: 200,
		intent,
		wake: wake
			? { detected: true, persona: wake.persona, command: wake.command }
			: { detected: false, persona: null, command: '' }
	};
}

describe('classifyCommand', () => {
	it('knows exact confirm and cancel phrases only', () => {
		expect(classifyCommand('Confirm.')).toBe('confirm');
		expect(classifyCommand('yes, do it')).toBe('confirm');
		expect(classifyCommand('go ahead')).toBe('confirm');
		expect(classifyCommand('cancel that')).toBe('cancel');
		expect(classifyCommand('never mind')).toBe('cancel');
		expect(classifyCommand("don't")).toBe('cancel');
	});

	it('treats anything longer or vaguer as other', () => {
		expect(classifyCommand('yes')).toBe('other');
		expect(classifyCommand('confirm the position size please')).toBe('other');
		expect(classifyCommand('buy gold and confirm')).toBe('other');
		expect(classifyCommand('')).toBe('other');
	});
});

describe('routeWake without a confirmation open', () => {
	it('ignores speech that has no wake phrase', () => {
		expect(routeWake(heard('what a lovely day'), ctx())).toEqual({ kind: 'ignore', reason: 'no-wake' });
	});

	it('runs the command after a wake phrase', () => {
		const r = routeWake(heard('hey jarvis what is gold at', { persona: 'jarvis', command: 'what is gold at' }), ctx());
		expect(r).toEqual({ kind: 'run', persona: 'jarvis', text: 'what is gold at', intent: 'other' });
	});

	it('reports the persona that was called, so the UI can switch', () => {
		const r = routeWake(heard('hey friday close gold', { persona: 'friday', command: 'close gold' }), ctx());
		expect(r).toMatchObject({ kind: 'run', persona: 'friday', text: 'close gold' });
	});

	it('arms on a bare wake phrase, then takes the next segment as the command', () => {
		const armed = routeWake(heard('hey friday', { persona: 'friday', command: '' }), ctx());
		expect(armed).toEqual({ kind: 'arm', persona: 'friday' });
		const next = routeWake(heard('show my positions'), ctx({ awaitingCommand: true }));
		expect(next).toEqual({ kind: 'run', persona: null, text: 'show my positions', intent: 'other' });
	});

	it('does not run an empty follow-up', () => {
		expect(routeWake(heard('  '), ctx({ awaitingCommand: true }))).toEqual({ kind: 'ignore', reason: 'empty' });
	});

	it('a spoken confirm with nothing pending is only words for the assistant', () => {
		const r = routeWake(heard('hey jarvis confirm', { persona: 'jarvis', command: 'confirm' }), ctx());
		expect(r).toMatchObject({ kind: 'run', intent: 'other' });
	});
});

describe('routeWake while a confirmation is open', () => {
	const pending = (over: Partial<WakeContext> = {}) => ctx({ pending: open, ...over });

	it('confirms only when the wake phrase came with it', () => {
		const r = routeWake(heard('hey jarvis confirm', { persona: 'jarvis', command: 'confirm' }), pending());
		expect(r).toEqual({ kind: 'run', persona: 'jarvis', text: 'confirm', intent: 'confirm' });
	});

	it('never confirms from bare speech, even if the server calls it a confirm', () => {
		expect(routeWake(heard('confirm', null, 'confirm'), pending())).toEqual({ kind: 'ignore', reason: 'pending' });
		expect(routeWake(heard('yes go ahead', null, 'confirm'), pending())).toEqual({ kind: 'ignore', reason: 'pending' });
	});

	it('cancels with or without the wake phrase', () => {
		expect(routeWake(heard('cancel', null, 'cancel'), pending())).toMatchObject({ kind: 'run', intent: 'cancel' });
		expect(routeWake(heard('hey jarvis cancel that', { persona: 'jarvis', command: 'cancel that' }), pending())).toMatchObject({
			kind: 'run',
			intent: 'cancel'
		});
	});

	it('ignores ordinary commands so they cannot start a second flow', () => {
		const r = routeWake(heard('hey jarvis buy gold', { persona: 'jarvis', command: 'buy gold' }), pending());
		expect(r).toEqual({ kind: 'ignore', reason: 'pending' });
	});

	it('ignores a wake phrase for another persona: the face must not change mid-flow', () => {
		const r = routeWake(heard('hey friday confirm', { persona: 'friday', command: 'confirm' }), pending());
		expect(r).toEqual({ kind: 'ignore', reason: 'other-persona' });
	});

	it('ignores everything while the confirm request is in flight', () => {
		const busy = ctx({ pending: { open: true, expired: false, busy: true } });
		expect(routeWake(heard('hey jarvis cancel', { persona: 'jarvis', command: 'cancel' }), busy)).toEqual({
			kind: 'ignore',
			reason: 'busy'
		});
	});
});
