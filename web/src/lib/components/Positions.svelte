<script lang="ts">
	import Panel from './Panel.svelte';
	import PollNote from './PollNote.svelte';
	import Menu, { type MenuItem } from './Menu.svelte';
	import PendingOrders from './PendingOrders.svelte';
	import { backend, digitsFor, positions } from '#lib/state/hud.svelte.ts';
	import { tradeActions } from '#lib/state/trade-actions.svelte.ts';
	import { decimalsOf, fmtPrice, fmtSigned, fmtVolume, tone } from '#lib/format.ts';
	import { countSide, groupBySymbol, halfVolume } from '#lib/trading.ts';
	import { symbolLabel } from '#lib/symbols.ts';
	import type { Position } from '#lib/api.ts';

	const list = $derived(positions.data ?? []);
	const totalPl = $derived(list.reduce((a, p) => a + p.profit, 0));
	const totalVol = $derived(list.reduce((a, p) => a + p.volume, 0));
	const blocked = $derived(backend.writeBlockedReason);
	const busy = $derived(tradeActions.busy !== null);

	/** Positions carry no `digits`; prefer the watchlist's, else infer from the numbers themselves. */
	function digits(p: Position): number {
		return (
			digitsFor(p.symbol) ??
			Math.max(decimalsOf(p.price_open), decimalsOf(p.price_current), p.price_open >= 100 ? 2 : 4)
		);
	}

	const batchItems = $derived.by<MenuItem[]>(() => {
		const buys = countSide(list, 'buy');
		const sells = countSide(list, 'sell');
		return [
			{ id: 'all', label: 'Close all', hint: String(list.length), danger: true },
			{ id: 'buys', label: 'Close buys', hint: String(buys), disabled: buys === 0 },
			{ id: 'sells', label: 'Close sells', hint: String(sells), disabled: sells === 0 },
			...groupBySymbol(list).map((g, i) => ({
				id: `sym:${g.symbol}`,
				label: `Close ${symbolLabel(list.find((p) => p.symbol === g.symbol) ?? g).primary}`,
				hint: String(g.count),
				divider: i === 0
			}))
		];
	});

	function onBatch(id: string) {
		if (id === 'all') void tradeActions.closeMany('all', { symbol: null, side: null, all: true });
		else if (id === 'buys') void tradeActions.closeMany('buys', { symbol: null, side: 'buy', all: false });
		else if (id === 'sells') void tradeActions.closeMany('sells', { symbol: null, side: 'sell', all: false });
		else if (id.startsWith('sym:')) {
			const symbol = id.slice(4);
			void tradeActions.closeMany(symbol, { symbol, side: null, all: false });
		}
	}

	function rowItems(p: Position): MenuItem[] {
		const half = halfVolume(p.volume);
		return [
			{
				id: 'half',
				label: 'Close half',
				hint: half ? fmtVolume(half) : 'too small',
				disabled: half === null
			}
		];
	}

	function onRow(p: Position, id: string) {
		if (id === 'half') {
			const half = halfVolume(p.volume);
			if (half) void tradeActions.closePosition(p.ticket, half);
		}
	}

	const btn =
		'h-7 border px-2.5 text-2xs font-medium tracking-widest uppercase transition-colors disabled:cursor-not-allowed disabled:opacity-40';
</script>

<Panel title="Positions" class="h-full">
	{#snippet meta()}
		{#if positions.data}<span>{list.length} open</span>{/if}
		<Menu
			label="Close positions"
			items={batchItems}
			onselect={onBatch}
			disabled={blocked !== null || busy || list.length === 0}
			title={blocked ?? (list.length === 0 ? 'No positions to close' : 'Close several positions')}
			class="{btn} border-down/60 text-down hover:bg-down/10"
		>
			{#snippet trigger()}Close… <span aria-hidden="true">▾</span>{/snippet}
		</Menu>
	{/snippet}

	<PollNote poller={positions} what="positions" rows={3} />

	{#if positions.data}
		{#if list.length === 0}
			<div class="p-4 text-xs text-mute" role="status">
				<p class="text-sm text-ink">No open positions</p>
				<p class="mt-1">Trades opened in MetaTrader 5 will appear here.</p>
			</div>
		{:else}
			<ul>
				{#each list as p (p.ticket)}
					{@const d = digits(p)}
					{@const label = symbolLabel(p)}
					<li class="border-b border-line/60 px-3 py-2.5" data-testid="position-row" data-ticket={p.ticket}>
						<div class="flex items-center justify-between gap-2">
							<div class="flex min-w-0 items-center gap-2">
								<span class="truncate font-medium" data-testid="position-symbol">{label.primary}</span>
								{#if label.secondary}
									<span class="num truncate text-2xs text-faint" title="Broker symbol" data-testid="position-broker-symbol">{label.secondary}</span>
								{/if}
								<span class="pill {p.side === 'buy' ? 't-up' : 't-down'} h-5 px-1.5">{p.side}</span>
								<span class="num text-xs text-mute">{fmtVolume(p.volume)}</span>
							</div>
							<span class="num text-sm font-medium t-{tone(p.profit)}">{fmtSigned(p.profit)}</span>
						</div>
						<div class="num mt-1 flex flex-wrap items-center justify-between gap-x-3 text-xs text-mute">
							<span>
								{fmtPrice(p.price_open, d)}
								<span aria-label="to" class="text-faint">→</span>
								<span class="text-ink">{fmtPrice(p.price_current, d)}</span>
							</span>
							<span class="flex gap-3">
								<span>SL <span class={p.sl === null ? 'text-faint' : 'text-ink'}>{p.sl === null ? '—' : fmtPrice(p.sl, d)}</span></span>
								<span>TP <span class={p.tp === null ? 'text-faint' : 'text-ink'}>{p.tp === null ? '—' : fmtPrice(p.tp, d)}</span></span>
							</span>
						</div>
						<div class="mt-2 flex items-center justify-end gap-1.5">
							<button
								type="button"
								class="{btn} border-down/60 text-down hover:bg-down/10"
								disabled={blocked !== null || busy}
								title={blocked ?? `Preview closing ticket ${p.ticket}`}
								aria-label="Close {label.primary} {p.side} {fmtVolume(p.volume)} (ticket {p.ticket})"
								onclick={() => tradeActions.closePosition(p.ticket)}
							>
								{tradeActions.busy === `close:${p.ticket}` ? 'Preparing…' : 'Close'}
							</button>
							<Menu
								label="More actions for ticket {p.ticket}"
								items={rowItems(p)}
								onselect={(id) => onRow(p, id)}
								disabled={blocked !== null || busy}
								title={blocked ?? 'More actions'}
								class="{btn} border-line-hi px-2 text-mute hover:border-accent hover:text-ink"
							>
								{#snippet trigger()}<span aria-hidden="true">⋯</span>{/snippet}
							</Menu>
						</div>
					</li>
				{/each}
			</ul>
		{/if}

		<PendingOrders {blocked} />

		{#if list.length > 0}
			<div class="sticky bottom-0 flex items-center justify-between border-t border-line-hi bg-deck/95 px-3 py-2 backdrop-blur">
				<span class="hud-title">Total · {fmtVolume(totalVol)} lots</span>
				<span class="num text-sm font-semibold t-{tone(totalPl)}">{fmtSigned(totalPl)}</span>
			</div>
		{/if}
	{/if}
</Panel>
