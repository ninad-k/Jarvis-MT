<script lang="ts">
	import Panel from './Panel.svelte';
	import PollNote from './PollNote.svelte';
	import { moves, watchlist } from '#lib/state/hud.svelte.ts';
	import { fmtPrice } from '#lib/format.ts';
	import { symbolLabel } from '#lib/symbols.ts';
	import { activeFor } from '#lib/alerts.ts';
	import { alerts } from '#lib/state/alerts.svelte.ts';
	import type { PriceTick } from '#lib/api.ts';

	const prices = $derived(watchlist.data?.prices ?? []);
	const errors = $derived(watchlist.data?.errors ?? []);
	const live = $derived(watchlist.view === 'ok');
	// The bell column goes away on a server without alerts.
	const cols = $derived(
		alerts.supported
			? 'grid-cols-[minmax(0,1fr)_auto_auto_1.75rem_1.5rem]'
			: 'grid-cols-[minmax(0,1fr)_auto_auto_2rem]'
	);

	const canonicalOf = (p: PriceTick) => symbolLabel(p).primary;
	const alertCount = (p: PriceTick) => activeFor(alerts.active, canonicalOf(p)).length;
</script>

<Panel title="Watchlist" class="h-full">
	{#snippet meta()}
		{#if watchlist.data}
			<span class="flex items-center gap-1.5 {live ? 'text-up' : 'text-warn'}">
				<span
					class="size-1.5 rounded-full bg-current {live ? 'animate-[pulse-dot_2s_ease-in-out_infinite]' : ''}"
					aria-hidden="true"
				></span>
				{live ? 'LIVE' : 'STALE'}
			</span>
		{/if}
	{/snippet}

	<PollNote poller={watchlist} what="prices" rows={6} />

	{#if watchlist.data}
		{#if prices.length === 0 && errors.length === 0}
			<p class="p-4 text-xs text-mute">No symbols configured. Set <code class="num">symbols.watchlist</code>.</p>
		{:else}
			<div
				class="grid {cols} gap-x-1.5 xl:gap-x-3 px-3 pt-2 pb-1 text-2xs tracking-widest text-faint uppercase"
				aria-hidden="true"
			>
				<span>Symbol</span><span class="text-right">Bid</span><span class="text-right">Ask</span>
				<span class="text-right">Spr</span>
				{#if alerts.supported}<span></span>{/if}
			</div>
			<ul class="pb-2">
				{#each prices as p, i (`${p.symbol}#${i}`)}
					{@const m = moves.map[p.symbol]}
					{@const label = symbolLabel(p)}
					<li
						class="relative grid {cols} items-center gap-x-1.5 border-t border-line/60 px-3 py-2 first:border-t-0 xl:gap-x-3"
					>
						<!-- The flash is its own layer, keyed so it restarts on every move; the cells (and the bell, which
						     holds focus) are never re-created. -->
						{#key m?.n}
							{#if m}
								<span class="pointer-events-none absolute inset-0 {m.dir === 'up' ? 'flash-up' : 'flash-down'}" aria-hidden="true"></span>
							{/if}
						{/key}
						<span class="flex min-w-0 items-center gap-1.5 font-medium">
							<span class="truncate" data-testid="wl-symbol">{label.primary}</span>
							{#if label.secondary}
								<span class="num truncate text-2xs font-normal text-faint" title="Broker symbol" data-testid="wl-broker-symbol">{label.secondary}</span>
							{/if}
							{#if m}
								<span class="text-[0.6rem] {m.dir === 'up' ? 't-up' : 't-down'}">
									<span aria-hidden="true">{m.dir === 'up' ? '▲' : '▼'}</span>
									<span class="sr-only">{m.dir === 'up' ? 'up' : 'down'}</span>
								</span>
							{/if}
						</span>
						<span class="num text-right text-xs xl:text-[0.8125rem] {m ? (m.dir === 'up' ? 't-up' : 't-down') : ''}">
							{fmtPrice(p.bid, p.digits)}
						</span>
						<span class="num text-right text-xs xl:text-[0.8125rem] {m ? (m.dir === 'up' ? 't-up' : 't-down') : ''}">
							{fmtPrice(p.ask, p.digits)}
						</span>
						<span class="num text-right text-xs text-mute" title="Spread in points">
							{p.spread_points}
						</span>
						{#if alerts.supported}
							{@const n = alertCount(p)}
							<button
								type="button"
								class="relative grid size-6 place-items-center border border-transparent text-mute transition-colors hover:border-accent hover:text-ink focus-visible:border-accent {n > 0 ? 'text-accent' : ''}"
								data-testid="alert-bell"
								data-bell={label.primary}
								aria-haspopup="dialog"
								aria-label="Set a price alert for {label.primary}{n > 0 ? ` (${n} active)` : ''}"
								title={n > 0 ? `${n} active alert${n === 1 ? '' : 's'}` : 'Set a price alert'}
								onclick={() => alerts.openFor({ symbol: label.primary, broker_symbol: p.symbol })}
							>
								<svg viewBox="0 0 24 24" class="size-3.5" fill={n > 0 ? 'currentColor' : 'none'} fill-opacity="0.25" stroke="currentColor" stroke-width="2" aria-hidden="true">
									<path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9" stroke-linejoin="round" />
									<path d="M10.3 21a1.9 1.9 0 0 0 3.4 0" stroke-linecap="round" />
								</svg>
							</button>
						{/if}
					</li>
				{/each}
				{#each errors as e, i (`${e.symbol}#${i}`)}
					<li class="border-t border-line/60 px-3 py-2">
						<div class="flex items-baseline justify-between gap-3">
							<span class="font-medium text-mute">{e.symbol}</span>
							<span class="truncate text-2xs text-down/90" title={e.message}>
								<span aria-hidden="true">△ </span>{e.message}
							</span>
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	{/if}
</Panel>
