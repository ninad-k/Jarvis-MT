<script lang="ts">
	import { provider } from '#lib/state/provider.svelte.ts';
	import { chat } from '#lib/state/chat.svelte.ts';

	let { class: klass = '' }: { class?: string } = $props();
	const providers = $derived(provider.providers);
</script>

<div class="flex items-center gap-2 {klass} sm:col-auto">
	<label for="llm-provider" class="hud-title max-[420px]:sr-only sm:max-xl:sr-only">LLM</label>
	<!-- Capped width: a long provider/model name must not push the top bar wider than the
	     window. The full list is also on the settings page. -->
	<select
		id="llm-provider"
		class="num h-7 min-w-0 flex-1 truncate border border-line bg-panel/70 px-2 text-2xs text-ink focus-visible:border-accent disabled:opacity-50 sm:w-44 sm:flex-none xl:w-56"
		disabled={providers.length === 0 || chat.pending}
		value={provider.selected ?? ''}
		onchange={(e) => void provider.select(e.currentTarget.value)}
		title={chat.pending ? 'Wait for the current reply to finish' : 'Language model used for new messages'}
	>
		{#if providers.length === 0}
			<option value="">—</option>
		{/if}
		{#each providers as p (p.name)}
			<option value={p.name} disabled={!p.available}>
				{p.name} · {p.model}{p.available ? '' : ' — API key not set'}
			</option>
		{/each}
	</select>
</div>
