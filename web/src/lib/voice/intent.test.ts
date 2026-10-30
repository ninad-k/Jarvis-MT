import { describe, expect, it } from 'vitest';
import { routeTranscript, type PendingState } from './intent';

const none: PendingState = { open: false, expired: false, busy: false };
const open: PendingState = { open: true, expired: false, busy: false };

describe('routeTranscript', () => {
	it('confirms a pending preview on a spoken confirm', () => {
		expect(routeTranscript({ text: 'confirm', intent: 'confirm' }, open)).toEqual({ kind: 'confirm' });
	});

	it('cancels a pending preview on a spoken cancel', () => {
		expect(routeTranscript({ text: 'cancel that', intent: 'cancel' }, open)).toEqual({ kind: 'cancel' });
	});

	it('still cancels after the countdown ran out (closes the stale preview)', () => {
		expect(routeTranscript({ text: 'cancel', intent: 'cancel' }, { ...open, expired: true })).toEqual({
			kind: 'cancel'
		});
	});

	it('does not confirm an expired preview', () => {
		expect(routeTranscript({ text: 'confirm', intent: 'confirm' }, { ...open, expired: true })).toEqual({
			kind: 'expired'
		});
	});

	it('ignores confirm/cancel while a confirm request is already running', () => {
		const busy = { ...open, busy: true };
		expect(routeTranscript({ text: 'confirm', intent: 'confirm' }, busy)).toEqual({ kind: 'empty' });
		expect(routeTranscript({ text: 'cancel', intent: 'cancel' }, busy)).toEqual({ kind: 'empty' });
	});

	it('sends confirm/cancel to chat when nothing is pending', () => {
		expect(routeTranscript({ text: 'confirm', intent: 'confirm' }, none)).toEqual({
			kind: 'chat',
			text: 'confirm'
		});
		expect(routeTranscript({ text: 'cancel', intent: 'cancel' }, none)).toEqual({
			kind: 'chat',
			text: 'cancel'
		});
	});

	it('sends other speech to chat even while a preview is pending', () => {
		expect(routeTranscript({ text: 'what is gold at', intent: 'other' }, open)).toEqual({
			kind: 'chat',
			text: 'what is gold at'
		});
	});

	it('trims the text and treats blank transcripts as empty', () => {
		expect(routeTranscript({ text: '  buy gold  ', intent: 'other' }, none)).toEqual({
			kind: 'chat',
			text: 'buy gold'
		});
		expect(routeTranscript({ text: '   ', intent: 'confirm' }, open)).toEqual({ kind: 'empty' });
		expect(routeTranscript({ text: '', intent: 'other' }, none)).toEqual({ kind: 'empty' });
	});
});
