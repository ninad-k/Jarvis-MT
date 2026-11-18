<script lang="ts">
	import { onMount } from 'svelte';
	import Panel from './Panel.svelte';
	import Segmented from './Segmented.svelte';
	import MarketCard from './MarketCard.svelte';
	import BriefingCard from './BriefingCard.svelte';
	import MarketTrackRecord from './MarketTrackRecord.svelte';
	import PrepareTradeDialog from './PrepareTradeDialog.svelte';
	import { clockTime, DISCLAIMER } from '#lib/market.ts';
	import { backend, digitsFor, watchlist } from '#lib/state/hud.svelte.ts';
	import { briefing } from '#lib/state/briefing.svelte.ts';
	import { market, type MarketTab } from '#lib/state/market.svelte.ts';
	import { symbolLabel } from '#lib/symbols.ts';

	/** From lg up the panel can hand the middle column back to the chat. */
	let { onclose }: { onclose?: () => void } = $props();
	const uid = $props.id();

	const tabs: { id: MarketTab; label: string }[] = [
		{ id: 'view', label: 'Analysis' },
		{ id: 'briefing', label: 'Briefing' },
		{ id: 'record', label: 'Track record' }
	];

	// Watchlist symbols by canonical name; the box also takes anything else the server can resolve.
	const choices = $derived.by(() => {
		const seen = new Set<string>();
		const out: string[] = [];
		for (const p of watchlist.data?.prices ?? []) {
			const name = symbolLabel(p).primary;
			if (!seen.has(name)) {
				seen.add(name);
				out.push(name);
			}
		}
		return out;
	});

	const view = $derived(market.view);
	const unavailable = $derived(
		backend.offline ? 'Arc MT is offline.' : backend.bridgeDown ? 'MT5 is not connected, so there are no prices to analyze.' : null
	);
	const hint = $derived(view ? digitsFor(view.broker_symbol) : undefined);

	// Keep "expires in" honest and hide Prepare trade the moment a view runs out.
	onMount(() => market.startClock());
	$effect(() => {
		const until = view ? Date.parse(view.valid_until) : NaN;
		if (Number.isNaN(until)) return;
		return market.tickAt(until);
	});

	function submit(e: Event) {
		e.preventDefault();
		if (market.loading || unavailable) return;
		void market.analyze();
	}

	/** A symbol in the briefing: show its full analysis. */
	function openFromBriefing(symbol: string) {
		market.setTab('view');
		market.symbolInput = symbol;
		if (!market.loading && !unavailable) void market.analyze(symbol);
	}

	function pick(symbol: string) {
		market.symbolInput = symbol;
		if (!market.loading && !unavailable) void market.analyze(symbol);
	}
</script>

<Panel title="Market view" class="h-full" scroll={false}>
	{#snippet meta()}
		{#if view && market.tab === 'view'}
			<span title="When this view was made">{clockTime(view.generated_at)}</span>
		{/if}
		{#if briefing.data && market.tab === 'briefing'}
			<span title="When this briefing was made">{clockTime(briefing.data.generated_at)}</span>
		{/if}
		{#if onclose}
			<button
				type="button"
				class="border border-line px-2 py-0.5 text-mute transition-colors hover:border-accent hover:text-ink max-lg:hidden"
				onclick={onclose}
			>
				Back to chat
			</button>
		{/if}
	{/snippet}

	<div class="shrink-0 border-b border-line px-3 py-2">
		<Segmented label="Market view sections" options={tabs} bind:value={() => market.tab, (t) => market.setTab(t)} />
	</div>

	<!-- Screen readers hear the progress and the result here; the card itself is not live. -->
	<p class="sr-only" role="status" aria-live="polite" data-testid="market-live">{market.announcement}</p>

	{#if market.tab === 'view'}
		<div id="panel-view" role="tabpanel" aria-labelledby="tab-view" class="min-h-0 flex-1 overflow-y-auto" aria-busy={market.loading}>
			<form class="space-y-2 border-b border-line p-3" onsubmit={submit}>
				<div class="flex items-end gap-2">
					<div class="min-w-0 flex-1">
						<label for="{uid}-symbol" class="hud-title block">Symbol</label>
						<input
							id="{uid}-symbol"
							class="input num mt-1"
							list="{uid}-symbols"
							bind:value={market.symbolInput}
							placeholder="XAUUSD, EURUSD, gold…"
							autocomplete="off"
							autocapitalize="none"
							spellcheck="false"
							maxlength="32"
							aria-describedby="{uid}-symbol-help"
						/>
						<datalist id="{uid}-symbols">
							{#each choices as c (c)}<option value={c}></option>{/each}
						</datalist>
					</div>
					<button
						type="submit"
						class="btn btn-primary h-10 shrink-0"
						disabled={market.loading || unavailable !== null || market.symbolInput.trim() === ''}
						title={unavailable ?? 'Analyze this symbol now'}
					>
						{market.loading ? 'Analyzing…' : 'Analyze'}
					</button>
				</div>
				<p id="{uid}-symbol-help" class="sr-only">Pick a watchlist symbol below or type any symbol or name, then press Analyze.</p>
				{#if choices.length}
					<div role="group" aria-label="Watchlist symbols" class="flex flex-wrap gap-1.5">
						{#each choices as c (c)}
							<button
								type="button"
								class="num h-7 border px-2 text-2xs transition-colors disabled:opacity-40 {view?.symbol === c
									? 'border-accent bg-accent/10 text-accent'
									: 'border-line-hi text-mute hover:border-accent hover:text-ink'}"
								aria-pressed={view?.symbol === c}
								disabled={market.loading || unavailable !== null}
								onclick={() => pick(c)}
							>
								{c}
							</button>
						{/each}
					</div>
				{/if}
				{#if unavailable}
					<p class="text-xs text-warn" role="status"><span aria-hidden="true">△ </span>{unavailable}</p>
				{/if}
			</form>

			<div>
				{#if view}
					{#if market.phase === 'error' && market.error}
						<p class="mx-3 mt-3 border border-down/60 bg-down/10 px-3 py-2 text-xs text-down" role="alert" data-testid="market-error">
							<span aria-hidden="true">△ </span>{market.error} The view below is the previous one.
						</p>
					{/if}
					<div class={market.loading ? 'opacity-60 transition-opacity' : 'transition-opacity'}>
						<MarketCard
							{view}
							now={market.now}
							digitsHint={hint ?? null}
							preparable={market.canPrepare}
							blockedReason={backend.writeBlockedReason}
							onprepare={() => market.openPrepare()}
						/>
					</div>
				{:else if market.loading}
					<div class="space-y-3 p-3" aria-label="Analyzing {market.requested ?? ''}">
						<div class="skeleton h-12"></div>
						<div class="skeleton h-10"></div>
						<div class="skeleton h-28"></div>
						<div class="skeleton h-24"></div>
					</div>
				{:else if market.phase === 'error'}
					<div class="flex flex-col items-start gap-2 p-4" role="alert" data-testid="market-error">
						<p class="flex items-center gap-2 text-sm font-medium text-warn"><span aria-hidden="true">△</span>Could not analyze</p>
						<p class="text-xs text-mute">{market.error}</p>
						<button type="button" class="btn" disabled={unavailable !== null} onclick={() => void market.analyze()}>Try again</button>
					</div>
				{:else}
					<div class="p-4 text-xs text-mute" data-testid="market-empty">
						<p class="text-sm text-ink">No analysis yet</p>
						<p class="mt-1">
							Pick a symbol and press Analyze. Arc MT reads the broker's prices, computes indicators on several
							timeframes, checks the economic calendar and, if a news provider is set up, the headlines. It never
							trades on its own.
						</p>
					</div>
				{/if}
			</div>
		</div>
	{:else if market.tab === 'briefing'}
		<div id="panel-briefing" role="tabpanel" aria-labelledby="tab-briefing" class="min-h-0 flex-1 overflow-y-auto" aria-busy={briefing.loading}>
			<p class="sr-only" role="status" aria-live="polite" data-testid="briefing-live">{briefing.announcement}</p>
			<div class="flex flex-wrap items-center justify-between gap-2 border-b border-line p-3">
				<p class="min-w-0 flex-1 text-xs text-mute">
					Your account, open positions, today's events and a computed view of every watchlist symbol.
				</p>
				<button
					type="button"
					class="btn btn-primary h-10 shrink-0"
					disabled={briefing.loading || unavailable !== null}
					title={unavailable ?? 'Build the briefing now'}
					onclick={() => void briefing.load()}
				>
					{briefing.loading ? 'Preparing…' : briefing.data ? 'Refresh briefing' : 'Brief me'}
				</button>
			</div>
			{#if unavailable}
				<p class="px-3 pt-3 text-xs text-warn" role="status"><span aria-hidden="true">△ </span>{unavailable}</p>
			{/if}

			{#if briefing.data}
				{#if briefing.phase === 'error' && briefing.error}
					<p class="mx-3 mt-3 border border-down/60 bg-down/10 px-3 py-2 text-xs text-down" role="alert" data-testid="briefing-error">
						<span aria-hidden="true">△ </span>{briefing.error} The briefing below is the previous one.
					</p>
				{/if}
				<div class={briefing.loading ? 'opacity-60 transition-opacity' : 'transition-opacity'}>
					<BriefingCard data={briefing.data} now={briefing.now} onopen={openFromBriefing} />
				</div>
			{:else if briefing.loading}
				<div class="space-y-3 p-3" aria-label="Preparing your briefing" data-testid="briefing-loading">
					<div class="skeleton h-12"></div>
					<div class="skeleton h-16"></div>
					<div class="skeleton h-24"></div>
					<div class="skeleton h-28"></div>
				</div>
			{:else if briefing.phase === 'error'}
				<div class="flex flex-col items-start gap-2 p-4" role="alert" data-testid="briefing-error">
					<p class="flex items-center gap-2 text-sm font-medium text-warn"><span aria-hidden="true">△</span>Could not prepare the briefing</p>
					<p class="text-xs text-mute">{briefing.error}</p>
					<button type="button" class="btn" disabled={unavailable !== null} onclick={() => void briefing.load()}>Try again</button>
				</div>
			{:else}
				<div class="p-4 text-xs text-mute" data-testid="briefing-empty">
					<p class="text-sm text-ink">No briefing yet</p>
					<p class="mt-1">
						Press Brief me for a short summary of where you stand and what is coming today. It reads your account and
						positions and never trades.
					</p>
				</div>
			{/if}
		</div>
	{:else}
		<div id="panel-record" role="tabpanel" aria-labelledby="tab-record" class="min-h-0 flex-1 overflow-y-auto">
			<MarketTrackRecord />
		</div>
	{/if}

	<p class="shrink-0 border-t border-line px-3 py-2 text-2xs text-mute" data-testid="market-disclaimer">
		{view?.disclaimer || DISCLAIMER}
	</p>
</Panel>

<PrepareTradeDialog />
