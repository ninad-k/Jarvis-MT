<script lang="ts">
	import type { BriefingDto } from '#lib/api.ts';
	import {
		accountLine,
		eventFigures,
		eventWhen,
		floatingTotal,
		isUnavailable,
		plText,
		positionsHeading,
		sortedEvents,
		utcTime,
		viewLine
	} from '#lib/briefing.ts';
	import { fmtPrice, fmtSigned, fmtVolume, tone } from '#lib/format.ts';
	import { biasInfo, clampConfidence, clockTime, importanceInfo, trendArrow } from '#lib/market.ts';
	import { symbolLabel } from '#lib/symbols.ts';

	let {
		data,
		now,
		onopen
	}: {
		data: BriefingDto;
		now: number;
		/** Open the full Market view for a symbol (canonical name). */
		onopen: (symbol: string) => void;
	} = $props();

	const ccy = $derived(data.account.currency);
	const total = $derived(floatingTotal(data.positions));
	const events = $derived(sortedEvents(data.events));
	const h = 'hud-title mb-1.5';
</script>

<article class="space-y-4 p-3" data-testid="briefing-card" aria-label="Daily briefing">
	<p class="text-sm leading-snug" data-testid="briefing-summary">{data.summary}</p>

	<!-- Account -->
	<section aria-label="Account">
		<h3 class={h}>Account</h3>
		<p class="num text-xs" data-testid="briefing-account">{accountLine(data.account)}</p>
		<p class="num mt-0.5 text-xs">
			<span class="text-mute">Floating P/L </span>
			<span class="font-semibold t-{tone(total)}" data-testid="briefing-total">{plText(total, ccy)}</span>
		</p>
	</section>

	<!-- Positions -->
	<section aria-label="Open positions">
		<h3 class={h}>{positionsHeading(data.positions.length)}</h3>
		{#if data.positions.length > 0}
			<ul class="divide-y divide-line border border-line text-xs">
				{#each data.positions as p (p.ticket)}
					{@const label = symbolLabel(p)}
					<li class="flex items-baseline justify-between gap-3 px-3 py-1.5" data-testid="briefing-position">
						<span class="min-w-0 truncate">
							<span class="font-medium">{label.primary}</span>
							<span class="num text-mute">{p.side === 'buy' ? 'Buy' : 'Sell'} {fmtVolume(p.volume)} @ {fmtPrice(p.price_open, p.price_open >= 100 ? 2 : 5)}</span>
						</span>
						<span class="num shrink-0 font-medium t-{tone(p.profit)}">{fmtSigned(p.profit)}</span>
					</li>
				{/each}
			</ul>
		{/if}
	</section>

	<!-- Events -->
	<section aria-label="Today's events">
		<h3 class={h}>Today's events <span class="font-normal normal-case text-mute">(UTC)</span></h3>
		{#if events.length === 0}
			<p class="text-xs text-mute" data-testid="briefing-no-events">No high-impact events for your watchlist currencies today.</p>
		{:else}
			<ul class="divide-y divide-line border border-line text-xs">
				{#each events as ev, i (`${ev.time}#${ev.title}#${i}`)}
					{@const imp = importanceInfo(ev.importance)}
					{@const figures = eventFigures(ev)}
					<li class="px-3 py-1.5" data-testid="briefing-event">
						<div class="flex flex-wrap items-baseline gap-x-2 gap-y-0.5">
							<span class="num text-mute">{utcTime(ev.time)}</span>
							<span class="num border border-line-hi px-1 text-2xs">{ev.currency}</span>
							<span class="min-w-0 flex-1 font-medium break-words">{ev.title}</span>
							<span class="border px-1 text-2xs {imp.cls}">{imp.label}<span class="sr-only"> impact</span></span>
						</div>
						<p class="num text-2xs text-mute">
							{eventWhen(ev, now)}{#if figures} · {figures}{/if}
						</p>
					</li>
				{/each}
			</ul>
		{/if}
	</section>

	<!-- Views -->
	<section aria-label="Symbol views">
		<h3 class={h}>Watchlist views</h3>
		{#if data.views.length === 0}
			<p class="text-xs text-mute">Your watchlist is empty, so there is nothing to analyze.</p>
		{:else}
			<ul class="space-y-2">
				{#each data.views as v (v.symbol)}
					{@const label = symbolLabel({ symbol: v.broker_symbol, canonical: v.symbol })}
					{@const bias = biasInfo(v.bias)}
					{@const unavailable = isUnavailable(v)}
					<li>
						{#if unavailable}
							<div class="border border-line px-3 py-2 text-xs" data-testid="briefing-view" data-symbol={v.symbol} data-unavailable="true">
								<p class="font-medium">{label.primary}</p>
								<p class="mt-0.5 text-warn"><span aria-hidden="true">△ </span>{viewLine(v)}</p>
							</div>
						{:else}
							<button
								type="button"
								class="block w-full border border-line px-3 py-2 text-left text-xs transition-colors hover:border-accent focus-visible:border-accent"
								data-testid="briefing-view"
								data-symbol={v.symbol}
								aria-label="Open the market view for {label.primary}: {viewLine(v)}"
								onclick={() => onopen(v.symbol)}
							>
								<span class="flex flex-wrap items-center gap-x-2 gap-y-1">
									<span class="text-sm font-semibold">{label.primary}</span>
									<span class="pill px-2 text-2xs font-semibold {bias.pill}" data-testid="briefing-bias">
										<span aria-hidden="true">{trendArrow(v.bias)}</span>
										{bias.label}
									</span>
									<span class="num text-mute" data-testid="briefing-confidence">{clampConfidence(v.confidence)} / 100</span>
									{#if v.stand_aside}
										<span class="border border-warn/70 bg-warn/10 px-1.5 text-2xs font-semibold text-warn" data-testid="briefing-stand-aside">
											<span aria-hidden="true">△ </span>Stand aside
										</span>
									{/if}
									<span class="ml-auto text-2xs text-accent" aria-hidden="true">Full view →</span>
								</span>
								{#if v.stand_aside && v.stand_aside_reason}
									<span class="mt-1 block text-warn">{v.stand_aside_reason}</span>
								{/if}
								<span class="mt-1 block text-mute">{v.summary}</span>
							</button>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
	</section>

	<p class="num text-2xs text-mute">Made at {clockTime(data.generated_at)}. Pick a symbol for its full market view.</p>
</article>
