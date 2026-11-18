<script lang="ts">
	import Panel from './Panel.svelte';
	import { alertTitle, newestFirst, triggeredAtText } from '#lib/alerts.ts';
	import { timeAgo } from '#lib/market.ts';
	import { alerts } from '#lib/state/alerts.svelte.ts';
	import { digitsForAny } from '#lib/state/hud.svelte.ts';

	// "3 min ago" needs a clock; minute resolution is plenty.
	let now = $state(Date.now());
	$effect(() => {
		const id = setInterval(() => (now = Date.now()), 30_000);
		return () => clearInterval(id);
	});

	const active = $derived(newestFirst(alerts.active));
	const recent = $derived(newestFirst(alerts.recent));
</script>

{#if alerts.supported}
	<Panel title="Alerts" class="max-h-52 shrink-0 lg:max-h-64">
		{#snippet meta()}
			{#if alerts.active.length > 0}
				<span data-testid="alert-count">{alerts.active.length} active</span>
			{/if}
		{/snippet}

		<p class="sr-only" role="status" aria-live="polite">{alerts.announcement}</p>

		{#if alerts.phase === 'error' && alerts.active.length === 0 && alerts.recent.length === 0}
			<div class="flex items-center justify-between gap-2 p-3 text-xs" role="status">
				<span class="text-warn"><span aria-hidden="true">△ </span>Could not load alerts. {alerts.error ?? ''}</span>
				<button type="button" class="btn" onclick={() => void alerts.refresh()}>Retry</button>
			</div>
		{:else if active.length === 0 && recent.length === 0}
			<p class="p-3 text-xs text-mute" data-testid="alerts-empty">
				{alerts.phase === 'ready' ? 'No alerts yet. Use the bell on a watchlist row to be told when a price is reached.' : 'Loading alerts…'}
			</p>
		{:else}
			{#if active.length > 0}
				<h3 class="hud-title px-3 pt-2 pb-1">Active</h3>
				<ul aria-label="Active alerts" class="pb-1">
					{#each active as a (a.id)}
						{@const d = digitsForAny(a.symbol)}
						<li class="flex items-center gap-2 border-t border-line/60 px-3 py-1.5 first:border-t-0" data-testid="alert-row" data-status="active" data-symbol={a.symbol}>
							<div class="min-w-0 flex-1">
								<p class="num truncate text-xs font-medium" data-testid="alert-title">{alertTitle(a, d)}</p>
								{#if a.note}<p class="truncate text-2xs text-mute" title={a.note}>{a.note}</p>{/if}
							</div>
							<button
								type="button"
								class="btn h-6 shrink-0 px-2 text-2xs"
								aria-label="Cancel alert: {alertTitle(a, d)}"
								disabled={alerts.cancelling.includes(a.id)}
								onclick={() => void alerts.cancel(a)}
							>
								Cancel
							</button>
						</li>
					{/each}
				</ul>
			{/if}
			{#if recent.length > 0}
				<h3 class="hud-title px-3 pt-2 pb-1">Recent</h3>
				<ul aria-label="Recent alerts" class="pb-2">
					{#each recent as a (a.id)}
						{@const d = digitsForAny(a.symbol)}
						<li class="border-t border-line/60 px-3 py-1.5 first:border-t-0" data-testid="alert-row" data-status="triggered" data-symbol={a.symbol}>
							<p class="num truncate text-xs"><span class="t-up" aria-hidden="true">✓ </span><span class="sr-only">Triggered: </span>{alertTitle(a, d)}</p>
							<p class="truncate text-2xs text-mute">
								{triggeredAtText(a, d)}{#if a.triggered_at}{a.trigger_price != null ? ' · ' : ''}{timeAgo(a.triggered_at, now)}{/if}{#if a.note} · {a.note}{/if}
							</p>
						</li>
					{/each}
				</ul>
			{/if}
		{/if}
	</Panel>
{/if}
