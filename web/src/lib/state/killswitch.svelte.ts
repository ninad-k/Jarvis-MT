import { api, type KillSwitchState } from '#lib/api.ts';
import { toApiError } from '#lib/errors.ts';

/**
 * Kill switch (SPEC rule 6). The truth lives on the server; this store mirrors it from status
 * polls, the WebSocket and the toggle's own response, whichever arrives first.
 */
class KillSwitchStore {
	state = $state<KillSwitchState>({ engaged: false, reason: null, changed_at_ms: null });
	busy = $state(false);

	get engaged(): boolean {
		return this.state.engaged;
	}

	/** From a status poll or a toggle response. A poll that was already in flight must not undo a newer change. */
	apply(next: KillSwitchState): void {
		const cur = this.state.changed_at_ms;
		if (next.changed_at_ms !== null && cur !== null && next.changed_at_ms < cur) return;
		this.state = next;
	}

	/** From the WebSocket. The event carries no server timestamp; the next status poll fills it in. */
	applyEvent(e: { engaged: boolean; reason: string | null }): void {
		this.state = { engaged: e.engaged, reason: e.reason, changed_at_ms: null };
	}

	/** Engage: immediate, one confirmation in the UI. Throws ApiError. */
	async engage(reason: string): Promise<void> {
		await this.set({ engaged: true, reason: reason.trim() || 'manual' });
	}

	/** Release: the server wants the TOTP code when auth is enabled. Throws ApiError. */
	async release(totp?: string): Promise<void> {
		await this.set({ engaged: false, ...(totp ? { totp } : {}) });
	}

	private async set(body: { engaged: boolean; reason?: string; totp?: string }): Promise<void> {
		this.busy = true;
		try {
			this.state = await api.setKillSwitch(body);
		} catch (e) {
			throw toApiError(e);
		} finally {
			this.busy = false;
		}
	}
}

export const killSwitch = new KillSwitchStore();
