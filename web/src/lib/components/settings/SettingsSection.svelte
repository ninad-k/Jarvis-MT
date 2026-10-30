<script lang="ts">
	import { onMount, type Snippet } from 'svelte';
	import { useDirtyRegistry, type Feedback } from '#lib/state/settings-context.ts';

	let {
		id,
		title,
		intro,
		children,
		extra,
		dirty = false,
		saving = false,
		canSave = true,
		saveLabel = 'Save',
		feedback = null,
		onsave,
		onrevert,
		hideActions = false
	}: {
		id: string;
		title: string;
		intro?: Snippet;
		children: Snippet;
		/** Extra buttons next to Save (for example "Reset to config"). */
		extra?: Snippet;
		dirty?: boolean;
		saving?: boolean;
		canSave?: boolean;
		saveLabel?: string;
		feedback?: Feedback | null;
		onsave?: () => void;
		onrevert?: () => void;
		hideActions?: boolean;
	} = $props();

	const registry = useDirtyRegistry();
	$effect(() => registry?.set(id, dirty));
	onMount(() => () => registry?.clear(id));
</script>

<section
	{id}
	aria-labelledby="{id}-title"
	data-testid="section-{id}"
	data-dirty={dirty}
	class="hud scroll-mt-4 p-4 sm:p-5"
>
	<h2 id="{id}-title" class="font-mono text-sm font-semibold tracking-[0.18em] text-ink uppercase">{title}</h2>
	{#if intro}
		<div class="mt-1.5 max-w-prose text-xs text-mute">{@render intro()}</div>
	{/if}

	<form
		class="mt-4 space-y-4"
		novalidate
		onsubmit={(e) => {
			e.preventDefault();
			if (!hideActions && canSave && !saving) onsave?.();
		}}
	>
		{@render children()}

		{#if !hideActions}
			<div class="flex flex-wrap items-center gap-2 border-t border-line pt-3">
				<button type="submit" class="btn btn-primary" disabled={!dirty || !canSave || saving} data-testid="{id}-save">
					{saving ? 'Saving…' : saveLabel}
				</button>
				{#if onrevert}
					<button type="button" class="btn" disabled={!dirty || saving} onclick={() => onrevert?.()}>Discard changes</button>
				{/if}
				{#if extra}{@render extra()}{/if}
				{#if dirty && !saving}
					<span class="text-2xs text-warn" data-testid="{id}-unsaved">Unsaved changes</span>
				{/if}
			</div>
		{/if}

		<div class="min-h-5" aria-live="polite" data-testid="{id}-feedback">
			{#if feedback}
				<p
					class="flex items-start gap-2 text-xs {feedback.kind === 'ok' ? 'text-up' : 'text-down'}"
					role={feedback.kind === 'error' ? 'alert' : 'status'}
					data-kind={feedback.kind}
				>
					<span aria-hidden="true">{feedback.kind === 'ok' ? '✓' : '△'}</span>
					<span class="min-w-0 break-words">{feedback.text}</span>
				</p>
			{/if}
		</div>
	</form>
</section>
