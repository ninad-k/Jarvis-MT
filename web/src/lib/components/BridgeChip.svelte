<script lang="ts">
	import { account, backend, status } from '#lib/state/hud.svelte.ts';

	const s = $derived(status.data);
	const b = $derived(s?.bridge ?? null);

	type Kind = 'connecting' | 'offline' | 'down' | 'up';
	const kind = $derived<Kind>(
		backend.offline ? 'offline' : !s ? 'connecting' : b?.connected ? 'up' : 'down'
	);

	const tradeMode = $derived(account.data?.trade_mode ?? b?.trade_mode ?? null);
	const label = $derived(
		{
			connecting: 'Connecting…',
			offline: 'API offline',
			down: 'MT5 disconnected',
			up: `MT5 connected${b?.latency_ms != null ? ` · ${b.latency_ms} ms` : ''}`
		}[kind]
	);
	const tone = $derived(
		{ connecting: 'text-mute', offline: 'text-down', down: 'text-down', up: 'text-up' }[kind]
	);
	const detail = $derived.by(() => {
		if (kind === 'offline')
			return status.error?.message ?? 'Cannot reach the Arc MT server. Retrying with back-off.';
		if (kind === 'down') return b?.error ?? 'The MT5 bridge is not connected.';
		if (kind === 'up' && b)
			return `EA ${b.ea_version ?? '?'} · protocol v${b.protocol_version ?? '?'} · terminal ${b.terminal_connected ? 'connected' : 'disconnected'}`;
		return 'Waiting for the first status reply.';
	});
	const realAccount = $derived(tradeMode === 'real');
</script>

<div class="flex items-center gap-2">
	<div class="group relative">
		<button
			type="button"
			class="num flex h-7 items-center gap-2 border border-line bg-panel/70 px-2.5 text-2xs tracking-wide {tone}"
			aria-describedby="bridge-detail"
			aria-label={label}
		>
			<span
				class="size-1.5 rounded-full bg-current {kind === 'connecting' ? 'animate-[pulse-dot_1s_ease-in-out_infinite]' : ''}"
				aria-hidden="true"
			></span>
			<span class="whitespace-nowrap max-sm:hidden">{label}</span>
			<span class="whitespace-nowrap sm:hidden" aria-hidden="true">{kind === 'up' && b?.latency_ms != null ? `${b.latency_ms} ms` : kind === 'up' ? 'MT5' : kind === 'offline' ? 'API' : kind === 'down' ? 'MT5' : '…'}</span>
		</button>
		<div
			id="bridge-detail"
			role="tooltip"
			class="num pointer-events-none absolute top-full right-0 z-30 mt-2 hidden w-64 max-w-[80vw] border border-line-hi bg-deck px-3 py-2 text-2xs leading-relaxed text-mute group-focus-within:block group-hover:block"
		>
			<p class="mb-1 {tone}">{label}</p>
			{detail}
		</div>
	</div>

	{#if tradeMode}
		<span
			class="pill max-sm:hidden {realAccount ? 't-down' : 'text-mute'}"
			title="MT5 account type reported by the bridge"
		>
			{tradeMode} acct
		</span>
	{/if}
</div>
