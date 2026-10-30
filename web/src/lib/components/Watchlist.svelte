<script lang="ts">
	import Panel from './Panel.svelte';
	import PollNote from './PollNote.svelte';
	import { moves, watchlist } from '#lib/state/hud.svelte.ts';
	import { fmtPrice } from '#lib/format.ts';

	const prices = $derived(watchlist.data?.prices ?? []);
	const errors = $derived(watchlist.data?.errors ?? []);
	const live = $derived(watchlist.view === 'ok');
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
				class="grid grid-cols-[minmax(0,1fr)_auto_auto_2rem] gap-x-2 xl:gap-x-3 px-3 pt-2 pb-1 text-2xs tracking-widest text-faint uppercase"
				aria-hidden="true"
			>
				<span>Symbol</span><span class="text-right">Bid</span><span class="text-right">Ask</span>
				<span class="text-right">Spr</span>
			</div>
			<ul class="pb-2">
				{#each prices as p (p.symbol)}
					{@const m = moves.map[p.symbol]}
					<li class="border-t border-line/60 first:border-t-0">
						{#key m?.n}
							<div
								class="grid grid-cols-[minmax(0,1fr)_auto_auto_2rem] items-center gap-x-2 xl:gap-x-3 px-3 py-2 {m
									? m.dir === 'up'
										? 'flash-up'
										: 'flash-down'
									: ''}"
							>
								<span class="flex items-center gap-1.5 truncate font-medium">
									{p.symbol}
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
							</div>
						{/key}
					</li>
				{/each}
				{#each errors as e (e.symbol)}
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
