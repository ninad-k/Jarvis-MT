<script lang="ts">
	import { digitsFor, orders } from '#lib/state/hud.svelte.ts';
	import { tradeActions } from '#lib/state/trade-actions.svelte.ts';
	import { decimalsOf, fmtPrice, fmtVolume } from '#lib/format.ts';
	import { symbolLabel } from '#lib/symbols.ts';

	let { blocked }: { blocked: string | null } = $props();

	const list = $derived(orders.data ?? []);
	const busy = $derived(tradeActions.busy !== null);
	const label = (t: string) => t.replace('_', ' ');
</script>

<!-- Hidden until the server reports at least one pending order (or if it has no orders endpoint). -->
{#if list.length > 0}
	<section aria-label="Pending orders" class="border-t border-line-hi">
		<h3 class="hud-title flex items-center justify-between px-3 py-2">
			<span>Pending orders</span><span class="num text-mute">{list.length}</span>
		</h3>
		<ul>
			{#each list as o (o.ticket)}
				{@const sl = symbolLabel(o)}
				{@const d = digitsFor(o.symbol) ?? Math.max(decimalsOf(o.price_open), o.price_open >= 100 ? 2 : 4)}
				<li class="flex items-center justify-between gap-2 border-t border-line/60 px-3 py-2" data-testid="order-row">
					<div class="min-w-0">
						<div class="flex items-center gap-2">
							<span class="truncate font-medium">{sl.primary}</span>
							{#if sl.secondary}
								<span class="num truncate text-2xs text-faint" title="Broker symbol">{sl.secondary}</span>
							{/if}
							<span class="pill h-5 px-1.5 {o.type.startsWith('buy') ? 't-up' : 't-down'}">{label(o.type)}</span>
						</div>
						<div class="num mt-0.5 text-xs text-mute">
							{fmtVolume(o.volume_current)} @ {fmtPrice(o.price_open, d)}
						</div>
					</div>
					<button
						type="button"
						class="h-7 shrink-0 border border-line-hi px-2.5 text-2xs font-medium tracking-widest text-mute uppercase transition-colors hover:border-down hover:text-down disabled:cursor-not-allowed disabled:opacity-40"
						disabled={blocked !== null || busy}
						title={blocked ?? `Preview cancelling order ${o.ticket}`}
						aria-label="Cancel {sl.primary} {label(o.type)} order (ticket {o.ticket})"
						onclick={() => tradeActions.cancelOrder(o.ticket)}
					>
						Cancel
					</button>
				</li>
			{/each}
		</ul>
	</section>
{/if}
