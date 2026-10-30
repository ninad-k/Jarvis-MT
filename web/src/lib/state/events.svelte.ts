import { MOCK, type WsEvent } from '#lib/api.ts';
import { reconnectDelay } from '#lib/backoff.ts';
import { killSwitch } from './killswitch.svelte';
import { refreshTrading, status } from './hud.svelte';
import { preview } from './preview.svelte';

export type EventsLink = 'idle' | 'connecting' | 'open' | 'retrying';

function isWsEvent(v: unknown): v is WsEvent {
	return typeof v === 'object' && v !== null && typeof (v as { type?: unknown }).type === 'string';
}

/**
 * Server push over `/api/ws`. Events make the HUD react faster than the polls; the polls keep
 * running as the fallback, so a dead socket only costs latency. Reconnects with back-off.
 */
class EventsStore {
	link = $state<EventsLink>('idle');
	attempts = $state(0);

	private ws: WebSocket | null = null;
	private timer: ReturnType<typeof setTimeout> | undefined;
	private stopMock: (() => void) | null = null;
	private wanted = false;

	start(): () => void {
		if (this.wanted) return () => this.stop();
		this.wanted = true;
		void this.connect();
		return () => this.stop();
	}

	stop(): void {
		this.wanted = false;
		clearTimeout(this.timer);
		this.stopMock?.();
		this.stopMock = null;
		if (this.ws) {
			this.ws.onclose = null;
			this.ws.close();
			this.ws = null;
		}
		this.link = 'idle';
	}

	/** Apply one event. Public so tests and the mock can feed it directly. */
	handle(e: WsEvent): void {
		switch (e.type) {
			case 'kill_switch':
				killSwitch.applyEvent(e);
				status.refresh();
				break;
			case 'preview_expired':
				preview.onExpiredEvent(e.token);
				break;
			case 'trade_executed':
			case 'positions_changed':
				refreshTrading();
				break;
		}
	}

	private async connect(): Promise<void> {
		this.link = 'connecting';
		if (MOCK) {
			const { onMockEvent } = await import('../mock');
			if (!this.wanted) return;
			this.stopMock = onMockEvent((e) => this.handle(e));
			this.link = 'open';
			return;
		}
		const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
		let ws: WebSocket;
		try {
			ws = new WebSocket(`${proto}//${location.host}/api/ws`);
		} catch {
			this.retry();
			return;
		}
		this.ws = ws;
		ws.onopen = () => {
			this.attempts = 0;
			this.link = 'open';
		};
		ws.onmessage = (m) => {
			try {
				const data: unknown = JSON.parse(String(m.data));
				if (isWsEvent(data)) this.handle(data);
			} catch {
				/* ignore a malformed frame */
			}
		};
		ws.onerror = () => ws.close();
		ws.onclose = () => {
			this.ws = null;
			if (this.wanted) this.retry();
		};
	}

	private retry(): void {
		this.link = 'retrying';
		const delay = reconnectDelay(this.attempts++);
		this.timer = setTimeout(() => void this.connect(), delay);
	}
}

export const events = new EventsStore();
