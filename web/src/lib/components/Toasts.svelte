<script lang="ts">
	import { toasts } from '#lib/state/toasts.svelte.ts';
</script>

<div
	class="pointer-events-none fixed inset-x-0 bottom-0 z-[70] flex flex-col items-center gap-2 p-3 pb-[max(0.75rem,env(safe-area-inset-bottom))]"
	role="region"
	aria-label="Notifications"
>
	{#each toasts.items as t (t.id)}
		<div
			class="rise-in pointer-events-auto flex max-w-md items-start gap-3 border px-3 py-2 text-xs shadow-[0_8px_24px_rgb(0_0_0/0.6)] {t.kind === 'error'
				? 'border-down/60 bg-deck text-down'
				: 'border-line-hi bg-deck text-ink'}"
			role={t.kind === 'error' ? 'alert' : 'status'}
		>
			<span class="min-w-0 break-words">{t.text}</span>
			<button
				type="button"
				class="shrink-0 text-mute hover:text-ink"
				aria-label="Dismiss notification"
				onclick={() => toasts.dismiss(t.id)}
			>
				✕
			</button>
		</div>
	{/each}
</div>
