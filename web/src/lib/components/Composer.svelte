<script lang="ts">
	import { chat, MAX_MESSAGE } from '#lib/state/chat.svelte.ts';
	import { backend } from '#lib/state/hud.svelte.ts';
	import { sendTurn } from '#lib/state/turn.svelte.ts';
	import { voice } from '#lib/state/voice.svelte.ts';
	import { primeAudio } from '#lib/voice/audio.ts';
	import { persona } from '#lib/state/persona.svelte.ts';
	import HandsFreeBar from './HandsFreeBar.svelte';
	import MicButton from './MicButton.svelte';

	let text = $state('');
	let input = $state<HTMLTextAreaElement>();
	let hadFocus = false;

	const trimmed = $derived(text.trim());
	const canSend = $derived(trimmed.length > 0 && trimmed.length <= MAX_MESSAGE && !chat.pending);
	const nearLimit = $derived(text.length > MAX_MESSAGE * 0.8);

	function grow() {
		if (!input) return;
		input.style.height = 'auto';
		input.style.height = `${Math.min(input.scrollHeight, 128)}px`;
	}

	async function submit() {
		if (!canSend) return;
		const msg = text;
		hadFocus = document.activeElement === input;
		text = '';
		queueMicrotask(grow);
		primeAudio(); // lets a spoken reply play (browsers need a gesture first)
		await sendTurn(msg);
	}

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
			e.preventDefault();
			void submit();
		}
	}

	// Return focus to the box after a reply, but only if the user was typing there.
	$effect(() => {
		if (!chat.pending && hadFocus) {
			hadFocus = false;
			input?.focus();
		}
	});
</script>

<form
	class="border-t border-line bg-deck/80 px-3 py-2.5 sm:px-4"
	onsubmit={(e) => {
		e.preventDefault();
		void submit();
	}}
>
	<HandsFreeBar />
	<div class="mx-auto flex max-w-3xl items-end gap-2">
		<div class="relative min-w-0 flex-1">
			<label for="chat-input" class="sr-only">Message {persona.name}</label>
			<textarea
				id="chat-input"
				bind:this={input}
				bind:value={text}
				oninput={grow}
				{onkeydown}
				rows="1"
				maxlength={MAX_MESSAGE}
				disabled={chat.pending}
				placeholder={chat.pending
					? `${persona.name} is thinking…`
					: backend.offline
						? 'Backend offline'
						: `Message ${persona.name}…`}
				enterkeyhint="send"
				autocomplete="off"
				class="block max-h-32 min-h-10 w-full resize-none border border-line-hi bg-void/70 px-3 py-2 text-base text-ink placeholder:text-faint focus-visible:border-accent focus-visible:outline-none disabled:opacity-60 sm:text-sm"
			></textarea>
			{#if nearLimit}
				<span
					class="num pointer-events-none absolute right-2 bottom-1 text-2xs {text.length >= MAX_MESSAGE
						? 't-down'
						: 'text-mute'}"
					aria-live="polite"
				>
					{text.length}/{MAX_MESSAGE}
				</span>
			{/if}
		</div>

		<MicButton />

		<button
			type="submit"
			disabled={!canSend}
			class="h-10 border border-accent bg-accent/15 px-4 text-xs font-medium tracking-widest text-accent uppercase transition-colors hover:bg-accent/25 disabled:cursor-not-allowed disabled:border-line disabled:bg-transparent disabled:text-faint"
		>
			Send
		</button>
	</div>
	{#if voice.message}
		<p class="mx-auto mt-2 max-w-3xl text-xs text-warn" role="alert" data-testid="voice-message">
			<span aria-hidden="true">△ </span>{voice.message}
		</p>
	{/if}
	<p class="sr-only">
		Enter sends, Shift plus Enter adds a new line. Outside this box, hold Space to talk.
	</p>
</form>
