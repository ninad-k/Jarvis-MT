<script lang="ts">
	import Panel from './Panel.svelte';
	import PollNote from './PollNote.svelte';
	import { digitsFor, positions } from '#lib/state/hud.svelte.ts';
	import { decimalsOf, fmtPrice, fmtSigned, fmtVolume, tone } from '#lib/format.ts';
	import type { Position } from '#lib/api.ts';

	const list = $derived(positions.data ?? []);
	const totalPl = $derived(list.reduce((a, p) => a + p.profit, 0));
	const totalVol = $derived(list.reduce((a, p) => a + p.volume, 0));

	/** Positions carry no `digits`; prefer the watchlist's, else infer from the numbers themselves. */
	function digits(p: Position): number {
		return (
			digitsFor(p.symbol) ??
			Math.max(decimalsOf(p.price_open), decimalsOf(p.price_current), p.price_open >= 100 ? 2 : 4)
		);
	}
</script>

<Panel title="Positions" class="h-full">
	{#snippet meta()}
		{#if positions.data}<span>{list.length} open</span>{/if}
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
					<li class="border-b border-line/60 px-3 py-2.5">
						<div class="flex items-center justify-between gap-2">
							<div class="flex min-w-0 items-center gap-2">
								<span class="truncate font-medium">{p.symbol}</span>
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
					</li>
				{/each}
			</ul>
			<div class="sticky bottom-0 flex items-center justify-between border-t border-line-hi bg-deck/95 px-3 py-2 backdrop-blur">
				<span class="hud-title">Total · {fmtVolume(totalVol)} lots</span>
				<span class="num text-sm font-semibold t-{tone(totalPl)}">{fmtSigned(totalPl)}</span>
			</div>
		{/if}
	{/if}
</Panel>
