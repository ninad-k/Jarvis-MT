<script lang="ts">
	import type { PollState } from '#lib/state/poller.svelte.ts';

	let { poller, what, rows = 4 }: { poller: PollState; what: string; rows?: number } = $props();

	const view = $derived(poller.view);
	const err = $derived(poller.error);
	const bridge = $derived(err?.code === 'bridge_unavailable' || err?.code === 'bridge_error');
</script>

{#if poller.data === null}
	{#if view === 'loading'}
		<div class="space-y-2 p-3" aria-busy="true" aria-label="Loading {what}">
			{#each Array(rows) as _, i (i)}
				<div class="skeleton h-9 rounded-[2px]"></div>
			{/each}
		</div>
	{:else if err}
		<div class="flex flex-col items-start gap-1 p-4" role="status">
			<p class="flex items-center gap-2 text-sm font-medium {view === 'offline' ? 'text-mute' : 'text-warn'}">
				<span aria-hidden="true">{view === 'offline' ? '◌' : '△'}</span>
				{#if view === 'offline'}
					Backend offline
				{:else if bridge}
					MT5 bridge unavailable
				{:else}
					Couldn't load {what}
				{/if}
			</p>
			<p class="text-xs text-mute">
				{#if view === 'offline'}
					Can't reach the Jarvis server. Retrying with back-off{poller.failures > 1
						? ` (attempt ${poller.failures})`
						: ''}.
				{:else}
					{err.message}
				{/if}
			</p>
		</div>
	{/if}
{:else if err}
	<p
		class="flex items-center gap-2 border-b border-line bg-warn/10 px-3 py-1.5 text-2xs text-warn"
		role="status"
	>
		<span aria-hidden="true">△</span>
		Stale: {err.isOffline ? 'backend offline' : err.message}
	</p>
{/if}
