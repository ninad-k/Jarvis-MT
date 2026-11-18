import type { WsEvent } from '#lib/api.ts';
import { reconnectDelay } from '#lib/backoff.ts';
import { alerts } from './alerts.svelte';
import { killSwitch } from './killswitch.svelte';
import { refreshTrading, status, watchlist } from './hud.svelte';
import { preview } from './preview.svelte';
import { refreshPreferences } from './refresh.svelte';
import { settings } from './settings.svelte';
import { symbolMap } from './symbols.svelte';

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
	/** Pages that want every event (the HUD), and pages that only follow settings and the symbol map. */
	private full = 0;
	private settingsPage = 0;

	start(): () => void {
		return this.hold('full');
	}

	/**
	 * Settings page: `symbol_map_changed` and `settings_changed` matter there (a change made from
	 * chat in another window); the HUD pollers are not running.
	 */
	watchSettingsPage(): () => void {
		return this.hold('settings');
	}

	private hold(kind: 'full' | 'settings'): () => void {
		if (kind === 'full') this.full++;
		else this.settingsPage++;
		if (!this.wanted) {
			this.wanted = true;
			void this.connect();
		}
		let released = false;
		return () => {
			if (released) return;
			released = true;
			if (kind === 'full') this.full = Math.max(0, this.full - 1);
			else this.settingsPage = Math.max(0, this.settingsPage - 1);
			if (this.full + this.settingsPage <= 0) this.stop();
		};
	}

	stop(): void {
		this.wanted = false;
		this.full = 0;
		this.settingsPage = 0;
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
		if (e.type === 'symbol_map_changed') {
			symbolMap.onChanged(e.broker);
			if (this.full > 0) {
				status.refresh();
				watchlist.refresh();
				refreshTrading();
			}
			return;
		}
		if (e.type === 'settings_changed') {
			// The settings page re-reads what it shows; the HUD also re-reads status (model list,
			// limits, persona name), the watchlist prices and the alerts, which chat can change too.
			if (this.full > 0) refreshPreferences();
			else {
				void settings.refresh();
				settings.bumpLimits();
			}
			return;
		}
		// With only the settings page listening, the HUD pollers are not running: nothing to refresh.
		if (this.full === 0) return;
		switch (e.type) {
			case 'kill_switch':
				killSwitch.applyEvent(e);
				status.refresh();
				break;
			case 'preview_expired':
				preview.onExpiredEvent(e.token);
				break;
			case 'alert_triggered':
				alerts.onTriggered(e);
				break;
			case 'trade_executed':
			case 'positions_changed':
				refreshTrading();
				break;
		}
	}

	private async connect(): Promise<void> {
		this.link = 'connecting';
		// Written out so the mock module is dropped from production builds (audit WEB-1).
		if (import.meta.env.VITE_MOCK === '1') {
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
