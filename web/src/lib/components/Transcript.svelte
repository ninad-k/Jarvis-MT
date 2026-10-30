<script lang="ts">
	import { tick } from 'svelte';
	import { chat } from '#lib/state/chat.svelte.ts';
	import { backend } from '#lib/state/hud.svelte.ts';
	import ToolChip from './ToolChip.svelte';

	let scroller = $state<HTMLElement>();
	let stick = true;

	const SUGGESTIONS = ["What's gold at?", 'Show my open positions', "What's my equity?"];

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
	{#if chat.messages.length === 0}
		<div class="mx-auto flex h-full max-w-md flex-col items-center justify-center gap-4 text-center">
			<div>
				<p class="text-sm text-ink">Ask Jarvis about your markets.</p>
				<p class="mt-1 text-xs text-mute">
					Read-only for now: prices, your account and open positions.
				</p>
			</div>
			<ul class="flex flex-wrap justify-center gap-2">
				{#each SUGGESTIONS as s (s)}
					<li>
						<button
							type="button"
							class="border border-line-hi bg-accent/5 px-3 py-1.5 text-xs text-mute transition-colors hover:border-accent hover:text-ink disabled:opacity-40"
							disabled={chat.pending || backend.offline}
							onclick={() => chat.send(s)}
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
							<span class="sr-only">You: </span>{m.text}
						</div>
						<span class="num px-1 text-2xs text-faint">{time(m.at)}</span>
					{:else if m.role === 'assistant'}
						<div class="max-w-[92%] border-l-2 border-accent bg-panel/80 py-2 pr-3 pl-3 text-sm break-words whitespace-pre-wrap">
							<span class="sr-only">Jarvis: </span>{m.text}
						</div>
						{#if m.toolCalls.length}
							<ul class="flex max-w-[92%] flex-wrap gap-1.5 pl-3" aria-label="Tools used">
								{#each m.toolCalls as c, i (i)}
									<ToolChip call={c} />
								{/each}
							</ul>
						{/if}
						<span class="num pl-3 text-2xs text-faint">{time(m.at)} · {m.provider}</span>
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
					Thinking…
				</li>
			{/if}
		</ol>
	{/if}
</div>
