<script lang="ts">
	import Panel from './Panel.svelte';
	import PollNote from './PollNote.svelte';
	import { account } from '#lib/state/hud.svelte.ts';
	import { fmtMoney, fmtPct, fmtSigned, tone } from '#lib/format.ts';

	const a = $derived(account.data);
	const marginTone = $derived(
		!a || a.margin <= 0 ? 'flat' : a.margin_level < 100 ? 'down' : a.margin_level < 200 ? 'warn' : 'flat'
	);
</script>

<Panel title="Account" scroll={false}>
	{#snippet meta()}
		{#if a}
			<span>{a.currency}</span><span aria-hidden="true">·</span><span>1:{a.leverage}</span>
		{/if}
	{/snippet}

	<PollNote poller={account} what="account" rows={2} />

	{#if a}
		<dl class="grid grid-cols-2 gap-px bg-line/70">
			<div class="bg-panel/90 px-3 py-2.5">
				<dt class="text-2xs tracking-widest text-faint uppercase">Balance</dt>
				<dd class="num text-lg leading-tight">{fmtMoney(a.balance)}</dd>
			</div>
			<div class="bg-panel/90 px-3 py-2.5">
				<dt class="text-2xs tracking-widest text-faint uppercase">Equity</dt>
				<dd class="num text-lg leading-tight text-accent">{fmtMoney(a.equity)}</dd>
			</div>
			<div class="bg-panel/90 px-3 py-2">
				<dt class="text-2xs tracking-widest text-faint uppercase">Floating P/L</dt>
				<dd class="num text-sm t-{tone(a.profit)}">{fmtSigned(a.profit)}</dd>
			</div>
			<div class="bg-panel/90 px-3 py-2">
				<dt class="text-2xs tracking-widest text-faint uppercase">Margin level</dt>
				<dd
					class="num text-sm {marginTone === 'down'
						? 't-down'
						: marginTone === 'warn'
							? 'text-warn'
							: ''}"
				>
					{a.margin > 0 ? fmtPct(a.margin_level) : '—'}
				</dd>
			</div>
			<div class="bg-panel/90 px-3 py-2">
				<dt class="text-2xs tracking-widest text-faint uppercase">Free margin</dt>
				<dd class="num text-sm">{fmtMoney(a.free_margin)}</dd>
			</div>
			<div class="bg-panel/90 px-3 py-2">
				<dt class="text-2xs tracking-widest text-faint uppercase">Margin used</dt>
				<dd class="num text-sm text-mute">{fmtMoney(a.margin)}</dd>
			</div>
		</dl>
	{/if}
</Panel>
