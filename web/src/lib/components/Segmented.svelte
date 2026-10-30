<script lang="ts" generics="T extends string">
	let {
		options,
		value = $bindable(),
		label,
		class: klass = ''
	}: {
		options: { id: T; label: string; badge?: string }[];
		value: T;
		label: string;
		class?: string;
	} = $props();

	function onkeydown(e: KeyboardEvent) {
		const i = options.findIndex((o) => o.id === value);
		let next = i;
		if (e.key === 'ArrowRight') next = (i + 1) % options.length;
		else if (e.key === 'ArrowLeft') next = (i - 1 + options.length) % options.length;
		else if (e.key === 'Home') next = 0;
		else if (e.key === 'End') next = options.length - 1;
		else return;
		e.preventDefault();
		value = options[next].id;
		(e.currentTarget as HTMLElement).querySelector<HTMLElement>(`[data-id="${value}"]`)?.focus();
	}
</script>

<div role="tablist" aria-label={label} class="flex border border-line bg-panel/70 p-0.5 {klass}" {onkeydown} tabindex="-1">
	{#each options as o (o.id)}
		<button
			type="button"
			role="tab"
			id="tab-{o.id}"
			data-id={o.id}
			aria-selected={value === o.id}
			aria-controls="panel-{o.id}"
			tabindex={value === o.id ? 0 : -1}
			class="flex-1 px-2 py-2 text-xs font-medium tracking-widest uppercase transition-colors {value === o.id
				? 'bg-accent/15 text-accent'
				: 'text-mute hover:text-ink'}"
			onclick={() => (value = o.id)}
		>
			{o.label}{#if o.badge}<span class="num ml-1.5 text-2xs opacity-70">{o.badge}</span>{/if}
		</button>
	{/each}
</div>
