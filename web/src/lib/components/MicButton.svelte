<script lang="ts">
	import { voice } from '#lib/state/voice.svelte.ts';
	import { backend } from '#lib/state/hud.svelte.ts';
	import { handsfree } from '#lib/state/handsfree.svelte.ts';
	import { persona } from '#lib/state/persona.svelte.ts';

	const handsFree = $derived(voice.mode === 'wake_word');

	const unavailable = $derived(
		backend.offline
			? 'Backend offline'
			: !backend.sttAvailable
				? 'Voice input is not available on this server'
				: null
	);
	const listening = $derived(handsFree ? handsfree.active : voice.listening);
	const busy = $derived(voice.phase === 'transcribing');

	function down(e: PointerEvent) {
		if (handsFree || e.button !== 0 || unavailable) return;
		(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
		voice.press();
	}

	function up() {
		if (!handsFree) voice.release();
	}

	// Keyboard activation (Enter/Space on the focused button) arrives as a click with detail 0.
	function click(e: MouseEvent) {
		if (unavailable) return;
		if (handsFree) {
			handsfree.toggle();
			return;
		}
		if (e.detail !== 0) return;
		voice.press();
		voice.release();
	}
</script>

<span class="group relative">
	<button
		type="button"
		data-testid="mic"
		aria-label={handsFree ? 'Hands-free listening' : 'Voice input'}
		aria-pressed={listening}
		aria-describedby="mic-tip"
		disabled={unavailable !== null || (busy && !handsFree)}
		class="relative grid size-10 place-items-center overflow-hidden border transition-colors select-none touch-none disabled:cursor-not-allowed disabled:opacity-50 {listening
			? 'border-down bg-down/15 text-down'
			: 'border-line-hi text-mute hover:border-accent hover:text-ink'}"
		onpointerdown={down}
		onpointerup={up}
		onpointercancel={up}
		onclick={click}
		oncontextmenu={(e) => e.preventDefault()}
	>
		{#if listening}
			<span
				class="absolute inset-0 rounded-full bg-down/30"
				style:transform="scale({0.4 + voice.level * 1.4})"
				aria-hidden="true"
			></span>
		{/if}
		<svg viewBox="0 0 24 24" class="relative size-5" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true">
			<rect x="9" y="3" width="6" height="11" rx="3" />
			<path d="M5 11a7 7 0 0 0 14 0M12 18v3" stroke-linecap="round" />
		</svg>
	</button>
	<span
		id="mic-tip"
		role="tooltip"
		class="pointer-events-none absolute right-0 bottom-full z-10 mb-2 hidden border border-line-hi bg-deck px-2 py-1 text-2xs whitespace-nowrap text-mute group-focus-within:block group-hover:block"
	>
		{unavailable ??
			(handsFree
				? listening
					? 'Stop hands-free listening'
					: `Start hands-free listening · say “${persona.wakeWord}”`
				: voice.mode === 'toggle'
					? listening
						? 'Tap to send · Esc cancels'
						: 'Tap to start, tap again to send · Space'
					: listening
						? 'Tap or release to send · Esc cancels'
						: 'Hold to talk, or tap to start and stop · Space')}
	</span>
</span>
