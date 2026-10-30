<script lang="ts">
	import { tick } from 'svelte';
	import { chat } from '#lib/state/chat.svelte.ts';
	import { persona } from '#lib/state/persona.svelte.ts';
	import { backend } from '#lib/state/hud.svelte.ts';
	import { sendTurn } from '#lib/state/turn.svelte.ts';
	import { voice } from '#lib/state/voice.svelte.ts';
	import ToolChip from './ToolChip.svelte';

	let scroller = $state<HTMLElement>();
	let stick = true;

	const SUGGESTIONS = [
		"What's gold at?",
		'Show my open positions',
		"What's my equity?",
		'Buy 0.1 gold with SL 2300'
	];

	function onScroll() {
		if (!scroller) return;
		stick = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 80;
	}

	const count = $derived(chat.messages.length);
	$effect(() => {
		void count;
		void chat.pending;
		const last = chat.messages.at(-1);
		if (last?.role === 'user') stick = true;
		if (!stick) return;
		void tick().then(() => scroller?.scrollTo({ top: scroller.scrollHeight }));
	});

	const time = (t: number) =>
		new Date(t).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' });
</script>

<!-- Scrollable log region: keyboard-focusable so it can be scrolled without a mouse. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div
	bind:this={scroller}
	onscroll={onScroll}
	class="min-h-0 flex-1 overflow-y-auto px-3 py-3 sm:px-4"
	role="log"
	aria-live="polite"
	aria-relevant="additions"
	aria-label="Conversation"
	tabindex="0"
>
	{#if chat.messages.length === 0 && !voice.busy}
		<div class="mx-auto flex h-full max-w-md flex-col items-center justify-center gap-4 text-center">
			<div>
				<p class="text-sm text-ink">Ask {persona.name} about your markets.</p>
				<p class="mt-1 text-xs text-mute">
					Prices, your account and positions, or ask for a trade. Nothing executes until you confirm it.
				</p>
			</div>
			<ul class="flex flex-wrap justify-center gap-2">
				{#each SUGGESTIONS as s (s)}
					<li>
						<button
							type="button"
							class="border border-line-hi bg-accent/5 px-3 py-1.5 text-xs text-mute transition-colors hover:border-accent hover:text-ink disabled:opacity-40"
							disabled={chat.pending || backend.offline}
							onclick={() => sendTurn(s)}
						>
							{s}
						</button>
					</li>
				{/each}
			</ul>
		</div>
	{:else}
		<ol class="mx-auto flex max-w-3xl flex-col gap-4">
			{#each chat.messages as m (m.id)}
				<li class="rise-in flex flex-col gap-1.5 {m.role === 'user' ? 'items-end' : 'items-start'}">
					{#if m.role === 'user'}
						<div
							class="max-w-[85%] border border-accent-dim/70 bg-accent/10 px-3 py-2 text-sm break-words whitespace-pre-wrap"
						>
							<span class="sr-only">You: </span>{#if m.voice}<svg viewBox="0 0 24 24" class="mr-1.5 inline size-3.5 -translate-y-px text-accent" fill="none" stroke="currentColor" stroke-width="2" role="img" aria-label="Spoken"><rect x="9" y="3" width="6" height="11" rx="3" /><path d="M5 11a7 7 0 0 0 14 0" stroke-linecap="round" /></svg>{/if}{m.text}
						</div>
						<span class="num px-1 text-2xs text-faint">{time(m.at)}</span>
					{:else if m.role === 'assistant'}
						<div class="max-w-[92%] border-l-2 border-accent bg-panel/80 py-2 pr-3 pl-3 text-sm break-words whitespace-pre-wrap">
							<span class="sr-only">{m.persona}: </span>{m.text}
						</div>
						{#if m.toolCalls.length}
							<ul class="flex max-w-[92%] flex-wrap gap-1.5 pl-3" aria-label="Tools used">
								{#each m.toolCalls as c, i (i)}
									<ToolChip call={c} />
								{/each}
							</ul>
						{/if}
						<span class="num pl-3 text-2xs text-faint">{time(m.at)} · {m.provider}</span>
					{:else if m.role === 'note'}
						<div
							class="max-w-[92%] border-l-2 py-1 pr-2 pl-3 text-xs break-words {m.tone === 'ok'
								? 'border-up/70 text-up'
								: m.tone === 'warn'
									? 'border-warn/70 text-warn'
									: 'border-down/70 text-down'}"
							data-testid="note"
						>
							<span aria-hidden="true">{m.tone === 'ok' ? '✓' : m.tone === 'warn' ? '△' : '✕'} </span>{m.text}
						</div>
					{:else}
						<div
							class="max-w-[92%] border border-down/50 bg-down/10 px-3 py-2 text-sm break-words text-down"
							role="alert"
						>
							<span aria-hidden="true">△ </span>{m.text}
							<span class="num mt-0.5 block text-2xs opacity-70">{m.code}</span>
						</div>
					{/if}
				</li>
			{/each}
			{#if voice.phase === 'listening' || voice.phase === 'starting'}
				<li class="flex items-center justify-end gap-2 pr-1 text-xs text-down" data-testid="listening-row">
					<span class="size-2 animate-[pulse-dot_1s_ease-in-out_infinite] rounded-full bg-down" aria-hidden="true"></span>
					{voice.phase === 'starting' ? 'Waiting for the microphone…' : `Listening… ${(voice.elapsedMs / 1000).toFixed(0)} s`}
				</li>
			{:else if voice.phase === 'transcribing'}
				<li class="flex items-center justify-end gap-2 pr-1 text-xs text-mute" data-testid="transcribing-row">
					<span class="size-2 animate-[pulse-dot_1s_ease-in-out_infinite] rounded-full bg-violet" aria-hidden="true"></span>
					Transcribing…
				</li>
			{/if}
			{#if chat.pending}
				<li class="flex items-center gap-2 pl-3 text-xs text-mute" aria-hidden="true">
					<span class="flex gap-1">
						{#each [0, 1, 2] as i (i)}
							<span
								class="size-1.5 rounded-full bg-violet animate-[blink_1s_ease-in-out_infinite]"
								style="animation-delay:{i * 0.15}s"
							></span>
						{/each}
					</span>
					{persona.name} is thinking…
				</li>
			{/if}
		</ol>
	{/if}
</div>
