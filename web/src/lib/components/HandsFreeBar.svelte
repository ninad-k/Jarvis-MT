<script lang="ts">
	import { onMount } from 'svelte';
	import { chat } from '#lib/state/chat.svelte.ts';
	import { handsfree } from '#lib/state/handsfree.svelte.ts';
	import { persona } from '#lib/state/persona.svelte.ts';
	import { speech } from '#lib/state/speech.svelte.ts';

	// Tell the store what the assistant is doing so it can stop hearing its own voice.
	$effect(() => {
		handsfree.setBusy({ speaking: speech.speaking, thinking: chat.pending });
	});

	// Leaving hands-free mode in the settings, or this page, releases the microphone.
	$effect(() => {
		if (!handsfree.wanted && handsfree.active) handsfree.stop();
	});
	onMount(() => () => handsfree.stop());

	const awake = $derived(handsfree.awake);
	const status = $derived.by(() => {
		if (awake) return { text: `${awake.name} is listening`, tone: 'on' as const };
		if (handsfree.pausedReason) return { text: handsfree.pausedReason, tone: 'paused' as const };
		switch (handsfree.phase) {
			case 'starting':
				return { text: 'Waiting for the microphone…', tone: 'paused' as const };
			case 'hearing':
				return { text: 'Hearing speech…', tone: 'on' as const };
			case 'transcribing':
				return { text: 'Working out what was said…', tone: 'on' as const };
			default:
				return { text: `Say “${persona.wakeWord}”, then your request`, tone: 'idle' as const };
		}
	});
</script>

{#if handsfree.wanted}
	<div
		class="mx-auto mb-2 flex max-w-3xl flex-wrap items-center gap-x-3 gap-y-1 border px-3 py-2 text-xs {handsfree.active
			? 'border-accent/60 bg-accent/10'
			: 'border-line bg-panel/60'}"
		data-testid="handsfree-bar"
		data-active={handsfree.active}
	>
		{#if handsfree.active}
			<span
				class="flex min-w-0 flex-1 items-center gap-2 text-ink"
				data-testid="handsfree-indicator"
				role="status"
				aria-live="polite"
			>
				<span class="relative grid size-4 shrink-0 place-items-center" aria-hidden="true">
					<span
						class="absolute inset-0 rounded-full bg-accent/40"
						style:transform="scale({0.5 + handsfree.level * 1.5})"
					></span>
					<span class="relative size-2 rounded-full bg-accent"></span>
				</span>
				<span class="font-medium">Hands-free listening</span>
				<span class="min-w-0 truncate text-mute" data-testid="handsfree-status">{status.text}</span>
			</span>
			<button
				type="button"
				data-testid="handsfree-stop"
				class="h-7 shrink-0 border border-line-hi px-3 text-2xs font-medium tracking-widest text-ink uppercase transition-colors hover:border-down hover:text-down"
				onclick={() => handsfree.stop()}
			>
				Stop listening
			</button>
		{:else}
			<span class="min-w-0 flex-1 text-mute" data-testid="handsfree-indicator" role="status">
				Hands-free is off. Start it, then say “{persona.wakeWord}”.
			</span>
			<button
				type="button"
				data-testid="handsfree-start"
				disabled={!handsfree.enabled || handsfree.phase === 'starting'}
				title={handsfree.unavailableReason ?? undefined}
				class="h-7 shrink-0 border border-accent bg-accent/15 px-3 text-2xs font-medium tracking-widest text-accent uppercase transition-colors hover:bg-accent/25 disabled:cursor-not-allowed disabled:border-line disabled:bg-transparent disabled:text-faint"
				onclick={() => void handsfree.start()}
			>
				Start listening
			</button>
		{/if}
		{#if handsfree.message}
			<p class="basis-full text-warn" role="alert"><span aria-hidden="true">△ </span>{handsfree.message}</p>
		{/if}
	</div>
{/if}
