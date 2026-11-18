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
				: t.kind === 'alert'
					? 'border-accent bg-deck text-ink'
					: 'border-line-hi bg-deck text-ink'}"
			role={t.kind === 'error' ? 'alert' : 'status'}
			aria-live={t.kind === 'error' ? 'assertive' : 'polite'}
			aria-atomic="true"
			data-kind={t.kind}
		>
			{#if t.kind === 'alert'}
				<svg viewBox="0 0 24 24" class="mt-px size-3.5 shrink-0 text-accent" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
					<path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9" stroke-linejoin="round" />
					<path d="M10.3 21a1.9 1.9 0 0 0 3.4 0" stroke-linecap="round" />
				</svg>
			{/if}
			<span class="min-w-0 break-words">{#if t.kind === 'alert'}<span class="sr-only">Price alert: </span>{/if}{t.text}</span>
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
