<script lang="ts">
	import { backend, status } from '#lib/state/hud.svelte.ts';
	import BridgeChip from './BridgeChip.svelte';
	import ProviderSelect from './ProviderSelect.svelte';

	const mode = $derived(status.data?.mode ?? null);
</script>

{#if backend.isLive}
	<div
		class="bg-down px-4 py-1.5 text-center text-xs font-semibold tracking-[0.12em] text-void"
		role="alert"
	>
		LIVE ACCOUNT — real money
	</div>
{/if}

<header
	class="relative z-20 grid shrink-0 grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 gap-y-2 border-b border-line bg-deck/80 px-4 pt-[max(0.5rem,env(safe-area-inset-top))] pb-2 backdrop-blur sm:flex sm:gap-x-4"
>
	<div class="flex min-w-0 items-center gap-3 sm:mr-auto">
		<h1 class="flex items-center gap-2 font-mono text-sm font-semibold tracking-[0.28em] whitespace-nowrap">
			<span class="grid size-5 place-items-center rounded-full border border-accent/70" aria-hidden="true">
				<span class="size-2 rounded-full bg-accent shadow-[0_0_8px_2px_rgb(34_211_238/0.7)]"></span>
			</span>
			JARVIS<span class="text-accent"> MT</span>
		</h1>
		{#if mode}
			<span
				class="pill {mode === 'demo' ? 'text-up' : 'bg-down/15 text-down'}"
				title={mode === 'demo' ? 'Demo account: no real money' : 'Live trading mode'}
			>
				<span class="size-1.5 rounded-full bg-current" aria-hidden="true"></span>
				{mode === 'demo' ? 'Demo' : 'Live'}
			</span>
		{/if}
	</div>

	<BridgeChip />
	<ProviderSelect class="col-span-2" />
</header>
