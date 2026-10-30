<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		title,
		meta,
		children,
		class: klass = '',
		scroll = true
	}: {
		title: string;
		meta?: Snippet;
		children: Snippet;
		class?: string;
		scroll?: boolean;
	} = $props();
</script>

<section class="hud flex min-h-0 flex-col {klass}" aria-label={title}>
	<header class="flex h-9 shrink-0 items-center justify-between gap-2 border-b border-line px-3">
		<h2 class="hud-title">{title}</h2>
		{#if meta}
			<div class="num flex items-center gap-2 text-2xs text-mute">{@render meta()}</div>
		{/if}
	</header>
	<div class="min-h-0 flex-1 {scroll ? 'overflow-y-auto' : 'flex flex-col'}">
		{@render children()}
	</div>
</section>
