<script lang="ts">
	import { persona } from '#lib/state/persona.svelte.ts';

	let { class: klass = '' }: { class?: string } = $props();

	const list = $derived(persona.list);
	const lock = $derived(persona.lockReason);
	const active = $derived(persona.activeId);
	/** Two personas fit a segmented toggle; more collapse to a dropdown. */
	const compact = $derived(list.length > 2);

	function onkeydown(e: KeyboardEvent) {
		if (e.key !== 'ArrowRight' && e.key !== 'ArrowLeft') return;
		const group = e.currentTarget as HTMLElement;
		const buttons = [...group.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')];
		const i = buttons.indexOf(document.activeElement as HTMLButtonElement);
		if (i < 0 || buttons.length < 2) return;
		e.preventDefault();
		buttons[(i + (e.key === 'ArrowRight' ? 1 : buttons.length - 1)) % buttons.length].focus();
	}
</script>

{#if list.length > 1}
	<div class="flex items-center gap-2 {klass}" data-testid="persona-toggle">
		{#if compact}
			<label for="persona-select" class="hud-title max-[420px]:sr-only">Persona</label>
			<select
				id="persona-select"
				class="num h-7 min-w-0 border border-line bg-panel/70 px-2 text-2xs text-ink focus-visible:border-accent disabled:opacity-50"
				disabled={lock !== null}
				value={active ?? ''}
				title={lock ?? 'Assistant persona'}
				onchange={(e) => void persona.select(e.currentTarget.value)}
			>
				{#each list as p (p.id)}
					<option value={p.id}>{p.name}</option>
				{/each}
			</select>
		{:else}
			<!-- A plain group of toggle buttons: Tab moves between them, arrows too. -->
			<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
			<div
				role="group"
				aria-label="Persona"
				aria-busy={persona.switching}
				class="flex h-7 border border-line bg-panel/70 p-0.5 {lock !== null ? 'opacity-60' : ''}"
				title={lock ?? undefined}
				{onkeydown}
			>
				{#each list as p (p.id)}
					{@const on = p.id === active}
					<button
						type="button"
						data-persona={p.id}
						aria-pressed={on}
						disabled={lock !== null}
						class="flex items-center gap-1.5 px-2.5 text-2xs font-medium tracking-wider uppercase transition-colors disabled:cursor-not-allowed {on
							? 'bg-accent/15 text-accent'
							: 'text-mute hover:text-ink'}"
						onclick={() => void persona.select(p.id)}
					>
						<span
							class="size-1.5 rounded-full"
							style:background-color={p.theme.accent}
							aria-hidden="true"
						></span>
						{p.name}
					</button>
				{/each}
			</div>
		{/if}
		{#if lock !== null}
			<span class="sr-only" role="status">{lock}</span>
		{/if}
	</div>
{/if}
