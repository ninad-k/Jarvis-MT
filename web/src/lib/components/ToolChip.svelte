<script lang="ts">
	import type { ToolCall } from '#lib/api.ts';
	import { summarizeArgs } from '#lib/format.ts';

	let { call }: { call: ToolCall } = $props();

	const args = $derived(summarizeArgs(call.arguments));
	const full = $derived(
		`${call.name}(${JSON.stringify(call.arguments)}) — ${call.ok ? 'ok' : 'failed'} in ${call.duration_ms} ms`
	);
</script>

<li
	class="num inline-flex max-w-full items-center gap-1.5 border px-2 py-0.5 text-2xs {call.ok
		? 'border-line-hi bg-accent/5 text-mute'
		: 'border-down/60 bg-down/10 text-down'}"
	title={full}
>
	<span aria-hidden="true" class={call.ok ? 't-up' : ''}>{call.ok ? '✓' : '✕'}</span>
	<span class="font-medium {call.ok ? 'text-accent' : ''}">{call.name}</span>
	{#if args}<span class="truncate opacity-80">({args})</span>{/if}
	<span class="shrink-0 opacity-70">{call.duration_ms} ms</span>
	<span class="sr-only">{call.ok ? 'succeeded' : 'failed'}</span>
</li>
