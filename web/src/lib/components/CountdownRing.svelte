<script lang="ts">
	let {
		fraction,
		seconds,
		urgent = false,
		expired = false,
		paused = false
	}: {
		fraction: number;
		seconds: number;
		urgent?: boolean;
		expired?: boolean;
		/** Voice turn: the window has not started yet. */
		paused?: boolean;
	} = $props();

	const R = 20;
	const C = 2 * Math.PI * R;
	const stroke = $derived(
		expired ? 'var(--color-down)' : urgent ? 'var(--color-down)' : seconds <= 10 ? 'var(--color-warn)' : 'var(--color-accent)'
	);
</script>

<div class="relative size-14 shrink-0" aria-hidden="true">
	<svg viewBox="0 0 48 48" class="size-full -rotate-90">
		<circle cx="24" cy="24" r={R} fill="none" stroke="var(--color-line)" stroke-width="4" />
		<circle
			cx="24"
			cy="24"
			r={R}
			fill="none"
			stroke={paused ? 'var(--color-line-hi)' : stroke}
			stroke-width="4"
			stroke-linecap="butt"
			stroke-dasharray={C}
			stroke-dashoffset={paused ? 0 : C * (1 - Math.max(0, Math.min(1, fraction)))}
			style="transition: stroke 200ms linear"
		/>
	</svg>
	<span
		class="num absolute inset-0 grid place-items-center text-lg font-semibold {expired || urgent
			? 't-down'
			: paused
				? 'text-mute'
				: ''}"
	>
		{#if paused}…{:else if expired}0{:else}{seconds}{/if}
	</span>
</div>
