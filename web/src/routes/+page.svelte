<script lang="ts">
	import { onMount } from 'svelte';
	import TopBar from '#lib/components/TopBar.svelte';
	import Orb from '#lib/components/Orb.svelte';
	import Watchlist from '#lib/components/Watchlist.svelte';
	import AccountStrip from '#lib/components/AccountStrip.svelte';
	import Positions from '#lib/components/Positions.svelte';
	import Panel from '#lib/components/Panel.svelte';
	import Transcript from '#lib/components/Transcript.svelte';
	import Composer from '#lib/components/Composer.svelte';
	import Segmented from '#lib/components/Segmented.svelte';
	import { positions, startPolling, watchlist } from '#lib/state/hud.svelte.ts';
	import { chat } from '#lib/state/chat.svelte.ts';

	type Tab = 'chat' | 'watchlist' | 'positions';
	let tab = $state<Tab>('chat');

	onMount(() => startPolling());

	const tabs = $derived([
		{ id: 'chat' as const, label: 'Chat' },
		{ id: 'watchlist' as const, label: 'Watchlist', badge: watchlist.data ? String(watchlist.data.prices.length) : undefined },
		{ id: 'positions' as const, label: 'Positions', badge: positions.data ? String(positions.data.length) : undefined }
	]);

	// Below lg only the selected tab's panel is shown; at lg everything is visible.
	const show = (t: Tab) => (tab === t ? '' : 'max-lg:hidden');
</script>

<div class="flex h-dvh flex-col">
	<TopBar />

	<main
		class="grid min-h-0 flex-1 grid-cols-1 grid-rows-[auto_auto_minmax(0,1fr)] gap-3 p-4 pb-[max(1rem,env(safe-area-inset-bottom))] lg:grid-cols-[18rem_minmax(0,1fr)_19rem] xl:grid-cols-[19rem_minmax(0,1fr)_24rem] lg:grid-rows-[minmax(14rem,1fr)_minmax(18rem,1.15fr)] lg:gap-4"
	>
		<!-- Orb -->
		<div class="h-[clamp(9rem,24dvh,15rem)] min-h-0 md:h-[clamp(12rem,30dvh,22rem)] lg:col-start-2 lg:row-start-1 lg:h-auto">
			<Orb />
		</div>

		<Segmented class="lg:hidden" label="Panels" options={tabs} bind:value={tab} />

		<!-- Watchlist -->
		<div
			id="panel-watchlist"
			role="tabpanel"
			aria-labelledby="tab-watchlist"
			class="min-h-0 lg:col-start-1 lg:row-span-2 lg:row-start-1 {show('watchlist')}"
		>
			<Watchlist />
		</div>

		<!-- Chat -->
		<div
			id="panel-chat"
			role="tabpanel"
			aria-labelledby="tab-chat"
			class="min-h-0 lg:col-start-2 lg:row-start-2 {show('chat')}"
		>
			<Panel title="Transcript" class="h-full" scroll={false}>
				{#snippet meta()}
					{#if chat.sessionId}
						<span title="Session {chat.sessionId}">session {chat.sessionId.slice(0, 8)}</span>
					{/if}
					{#if chat.messages.length > 0}
						<button
							type="button"
							class="border border-line px-2 py-0.5 text-mute transition-colors hover:border-accent hover:text-ink disabled:opacity-40"
							disabled={chat.pending}
							onclick={() => chat.reset()}
						>
							New chat
						</button>
					{/if}
				{/snippet}
				<Transcript />
				<Composer />
			</Panel>
		</div>

		<!-- Account + positions -->
		<div
			id="panel-positions"
			role="tabpanel"
			aria-labelledby="tab-positions"
			class="min-h-0 flex-col gap-3 lg:col-start-3 lg:row-span-2 lg:row-start-1 lg:flex lg:gap-4 {tab === 'positions' ? 'flex' : 'max-lg:hidden'}"
		>
			<AccountStrip />
			<div class="min-h-0 flex-1"><Positions /></div>
		</div>
	</main>
</div>
