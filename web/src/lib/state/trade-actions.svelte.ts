import { api, type ClosePositionsRequest, type PreviewDto } from '#lib/api.ts';
import { preview, reportActionError } from './preview.svelte';

/**
 * UI buttons that start a close or cancel. They only ever ask the server for a preview
 * (docs/api.md: PreviewDto); nothing is executed until the confirmation modal is confirmed.
 */
class TradeActions {
	/** Key of the request in flight (`close:123`, `batch:XAUUSD`...), so its button can show it. */
	busy = $state<string | null>(null);

	closePosition(ticket: number, volume: number | null = null): Promise<void> {
		return this.run(`close:${ticket}${volume ? `:${volume}` : ''}`, () => api.closePosition(ticket, volume));
	}

	closeMany(key: string, req: ClosePositionsRequest): Promise<void> {
		return this.run(`batch:${key}`, () => api.closePositions(req));
	}

	cancelOrder(ticket: number): Promise<void> {
		return this.run(`cancel:${ticket}`, () => api.cancelOrder(ticket));
	}

	private async run(key: string, fn: () => Promise<PreviewDto>): Promise<void> {
		if (this.busy !== null) return;
		this.busy = key;
		try {
			preview.open(await fn());
		} catch (e) {
			reportActionError(e);
		} finally {
			this.busy = null;
		}
	}
}

export const tradeActions = new TradeActions();
