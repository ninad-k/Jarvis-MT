<script lang="ts">
	import { sourceInfo } from '#lib/symbols.ts';

	/** Where a symbol mapping came from, with a tooltip that says what that means. */
	let { source, id }: { source: string; id: string } = $props();

	const info = $derived(sourceInfo(source));
	let hidden = $state(false);
</script>

<span class="group relative inline-block">
	<button
		type="button"
		class="pill h-5 cursor-help px-1.5 {info.tone}"
		aria-describedby={id}
		data-testid="source-badge"
		data-source={source}
		onmouseenter={() => (hidden = false)}
		onfocus={() => (hidden = false)}
		onkeydown={(e) => {
			// Esc dismisses the tooltip without moving focus (WCAG 1.4.13).
			if (e.key === 'Escape') hidden = true;
		}}
	>
		{info.label}
	</button>
	<span
		{id}
		role="tooltip"
		class="pointer-events-none absolute top-full left-0 z-30 mt-1.5 w-56 max-w-[70vw] border border-line-hi bg-deck px-3 py-2 text-2xs leading-relaxed font-normal tracking-normal text-mute normal-case {hidden
			? 'hidden'
			: 'hidden group-focus-within:block group-hover:block'}"
	>
		{info.tip}
	</span>
</span>
