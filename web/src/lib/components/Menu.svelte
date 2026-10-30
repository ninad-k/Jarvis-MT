<script lang="ts" module>
	export interface MenuItem {
		id: string;
		label: string;
		/** Small right-aligned text, e.g. a count. */
		hint?: string;
		disabled?: boolean;
		danger?: boolean;
		/** Renders as a checkbox item with this state. */
		checked?: boolean;
		/** Draw a divider above this item. */
		divider?: boolean;
	}
</script>

<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		items,
		label,
		onselect,
		trigger,
		disabled = false,
		title,
		align = 'right',
		class: klass = ''
	}: {
		items: MenuItem[];
		label: string;
		onselect: (id: string) => void;
		trigger: Snippet;
		disabled?: boolean;
		title?: string;
		align?: 'left' | 'right';
		class?: string;
	} = $props();

	let open = $state(false);
	let button = $state<HTMLButtonElement>();
	let list = $state<HTMLDivElement>();
	let pos = $state({ top: 0, left: 0, right: 0, maxH: 320 });
	const uid = $props.id();

	/** Render the list under <body>: panels use backdrop-filter, which would trap `position: fixed`. */
	function portal(node: HTMLElement) {
		document.body.appendChild(node);
		return { destroy: () => node.remove() };
	}

	function place() {
		if (!button) return;
		const r = button.getBoundingClientRect();
		const below = window.innerHeight - r.bottom - 12;
		const above = r.top - 12;
		const flip = below < 200 && above > below;
		pos = {
			top: flip ? Math.max(8, r.top - Math.min(320, above) - 4) : r.bottom + 4,
			left: r.left,
			right: Math.max(8, window.innerWidth - r.right),
			maxH: Math.min(320, flip ? above : below)
		};
	}

	function enabledItems(): HTMLElement[] {
		return [...(list?.querySelectorAll<HTMLElement>('[role^="menuitem"]:not([aria-disabled="true"])') ?? [])];
	}

	async function show(focusLast = false) {
		if (disabled) return;
		place();
		open = true;
		await Promise.resolve();
		requestAnimationFrame(() => {
			const els = enabledItems();
			(focusLast ? els.at(-1) : els[0])?.focus();
		});
	}

	function hide(refocus = true) {
		if (!open) return;
		open = false;
		if (refocus) button?.focus();
	}

	function pick(item: MenuItem) {
		if (item.disabled) return;
		hide();
		onselect(item.id);
	}

	function onListKey(e: KeyboardEvent) {
		const els = enabledItems();
		const i = els.indexOf(document.activeElement as HTMLElement);
		if (e.key === 'ArrowDown') els[(i + 1) % els.length]?.focus();
		else if (e.key === 'ArrowUp') els[(i - 1 + els.length) % els.length]?.focus();
		else if (e.key === 'Home') els[0]?.focus();
		else if (e.key === 'End') els.at(-1)?.focus();
		else if (e.key === 'Escape') hide();
		else if (e.key === 'Tab') hide(false);
		else return;
		e.preventDefault();
		e.stopPropagation();
	}

	function onButtonKey(e: KeyboardEvent) {
		if (e.key === 'ArrowDown') {
			e.preventDefault();
			void show();
		} else if (e.key === 'ArrowUp') {
			e.preventDefault();
			void show(true);
		}
	}

	$effect(() => {
		if (!open) return;
		const away = (e: PointerEvent) => {
			const t = e.target as Node;
			if (!list?.contains(t) && !button?.contains(t)) hide(false);
		};
		const move = () => hide(false);
		document.addEventListener('pointerdown', away, true);
		window.addEventListener('resize', move);
		window.addEventListener('scroll', move, true);
		return () => {
			document.removeEventListener('pointerdown', away, true);
			window.removeEventListener('resize', move);
			window.removeEventListener('scroll', move, true);
		};
	});

	$effect(() => {
		if (disabled && open) open = false;
	});
</script>

<button
	bind:this={button}
	type="button"
	class={klass}
	aria-haspopup="menu"
	aria-expanded={open}
	aria-controls={open ? `${uid}-menu` : undefined}
	aria-label={label}
	{disabled}
	{title}
	onclick={() => (open ? hide() : void show())}
	onkeydown={onButtonKey}
>
	{@render trigger()}
</button>

{#if open}
	<div
		bind:this={list}
		use:portal
		id="{uid}-menu"
		role="menu"
		aria-label={label}
		tabindex="-1"
		class="fixed z-50 min-w-44 overflow-y-auto border border-line-hi bg-deck py-1 shadow-[0_8px_24px_rgb(0_0_0/0.6)]"
		style:top="{pos.top}px"
		style:max-height="{pos.maxH}px"
		style:left={align === 'left' ? `${pos.left}px` : undefined}
		style:right={align === 'right' ? `${pos.right}px` : undefined}
		onkeydown={onListKey}
	>
		{#each items as item (item.id)}
			{#if item.divider}<div role="separator" class="my-1 border-t border-line"></div>{/if}
			<button
				type="button"
				role={item.checked === undefined ? 'menuitem' : 'menuitemcheckbox'}
				aria-checked={item.checked}
				aria-disabled={item.disabled ? 'true' : undefined}
				tabindex="-1"
				class="flex w-full items-center justify-between gap-4 px-3 py-2 text-left text-xs whitespace-nowrap transition-colors focus-visible:bg-accent/15 focus-visible:outline-none {item.disabled
					? 'cursor-not-allowed text-faint'
					: item.danger
						? 'text-down hover:bg-down/10 focus-visible:bg-down/15'
						: 'text-ink hover:bg-accent/10'}"
				onclick={() => pick(item)}
			>
				<span class="flex items-center gap-2">
					{#if item.checked !== undefined}
						<span class="inline-block w-3 text-accent" aria-hidden="true">{item.checked ? '✓' : ''}</span>
					{/if}
					{item.label}
				</span>
				{#if item.hint}<span class="num text-2xs text-mute">{item.hint}</span>{/if}
			</button>
		{/each}
	</div>
{/if}
