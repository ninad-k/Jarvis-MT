<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type AuditEntry } from '#lib/api.ts';
	import { describeError, toApiError } from '#lib/errors.ts';
	import { auditMs, payloadSummary } from '#lib/format.ts';

	const PAGE = 50;

	let entries = $state<AuditEntry[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let more = $state(true);
	let kind = $state('all');
	let open = $state<Set<number>>(new Set());

	const kinds = $derived(['all', ...new Set(entries.map((e) => e.kind))].sort((a, b) => (a === 'all' ? -1 : b === 'all' ? 1 : a.localeCompare(b))));
	const shown = $derived(kind === 'all' ? entries : entries.filter((e) => e.kind === kind));

	async function load(reset: boolean) {
		loading = true;
		error = null;
		try {
			const before = reset ? null : (entries.at(-1)?.id ?? null);
			const res = await api.audit({ limit: PAGE, beforeId: before });
			entries = reset ? res.entries : [...entries, ...res.entries];
			more = res.entries.length === PAGE;
			if (reset) open = new Set();
		} catch (e) {
			error = describeError(toApiError(e));
		} finally {
			loading = false;
		}
	}

	onMount(() => void load(true));

	function toggle(id: number) {
		const next = new Set(open);
		if (next.has(id)) next.delete(id);
		else next.add(id);
		open = next;
	}

	const dateOf = (ts: number | string) => new Date(auditMs(ts));
	const time = (ts: number | string) =>
		dateOf(ts).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit', second: '2-digit' });
	const day = (ts: number | string) =>
		dateOf(ts).toLocaleDateString('en-GB', { day: '2-digit', month: 'short' });

	function tone(k: string): string {
		if (/fail|reject|kill|unknown/.test(k)) return 'text-down';
		if (/confirm|trade_result|preview/.test(k)) return 'text-accent';
		if (/login|voice/.test(k)) return 'text-violet';
		return 'text-mute';
	}
</script>

<svelte:head><title>Audit log · Jarvis MT</title></svelte:head>

<div class="flex h-dvh flex-col">
	<header
		class="flex shrink-0 flex-wrap items-center gap-x-4 gap-y-2 border-b border-line bg-deck/80 px-4 pt-[max(0.5rem,env(safe-area-inset-top))] pb-2 backdrop-blur"
	>
		<a href="/" class="num text-xs text-mute hover:text-ink">← HUD</a>
		<h1 class="font-mono text-sm font-semibold tracking-[0.2em] uppercase">Audit log</h1>
		<div class="ml-auto flex items-center gap-2">
			<label for="audit-kind" class="hud-title">Kind</label>
			<select
				id="audit-kind"
				bind:value={kind}
				class="num h-7 border border-line bg-panel/70 px-2 text-2xs text-ink focus-visible:border-accent"
			>
				{#each kinds as k (k)}<option value={k}>{k}</option>{/each}
			</select>
			<button
				type="button"
				class="h-7 border border-line px-2.5 text-2xs tracking-widest text-mute uppercase hover:border-accent hover:text-ink disabled:opacity-40"
				disabled={loading}
				onclick={() => load(true)}
			>
				Refresh
			</button>
		</div>
	</header>

	<main class="min-h-0 flex-1 overflow-y-auto p-4">
		<div class="mx-auto max-w-5xl">
			{#if error}
				<div class="mb-3 flex items-center justify-between gap-3 border border-down/60 bg-down/10 px-3 py-2 text-xs text-down" role="alert">
					<span>{error}</span>
					<button type="button" class="underline" onclick={() => load(entries.length === 0)}>Retry</button>
				</div>
			{/if}

			{#if loading && entries.length === 0}
				<div class="space-y-2" aria-busy="true" aria-label="Loading audit entries">
					{#each Array(8) as _, i (i)}<div class="skeleton h-10"></div>{/each}
				</div>
			{:else if shown.length === 0 && !error}
				<p class="p-6 text-center text-sm text-mute" role="status">No audit entries{kind === 'all' ? '' : ` of kind “${kind}”`}.</p>
			{:else}
				<ol class="hud divide-y divide-line/70" data-testid="audit-list">
					{#each shown as e (e.id)}
						{@const isOpen = open.has(e.id)}
						<li data-testid="audit-row">
							<button
								type="button"
								class="grid w-full grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-3 gap-y-0.5 px-3 py-2 text-left hover:bg-accent/5 sm:grid-cols-[4.5rem_9rem_minmax(0,1fr)_auto]"
								aria-expanded={isOpen}
								aria-controls="audit-{e.id}"
								onclick={() => toggle(e.id)}
							>
								<span class="num text-2xs text-mute">
									<span class="block">{time(e.ts)}</span><span class="text-faint">{day(e.ts)}</span>
								</span>
								<span class="num text-xs font-medium {tone(e.kind)}">{e.kind}</span>
								<span class="num col-span-2 min-w-0 truncate text-xs text-mute sm:col-span-1">{payloadSummary(e.payload)}</span>
								<span class="num hidden text-2xs text-faint sm:block">
									#{e.id}{e.session_id ? ` · ${e.session_id.slice(0, 8)}` : ''}
									<span aria-hidden="true"> {isOpen ? '▾' : '▸'}</span>
								</span>
							</button>
							{#if isOpen}
								<pre
									id="audit-{e.id}"
									class="num overflow-x-auto border-t border-line/70 bg-void/60 px-3 py-2 text-2xs leading-relaxed text-ink">{JSON.stringify(e.payload, null, 2)}</pre>
							{/if}
						</li>
					{/each}
				</ol>

				<div class="flex justify-center py-4">
					{#if more}
						<button
							type="button"
							class="h-9 border border-line-hi px-4 text-xs tracking-widest text-mute uppercase hover:border-accent hover:text-ink disabled:opacity-40"
							disabled={loading}
							onclick={() => load(false)}
						>
							{loading ? 'Loading…' : 'Load older'}
						</button>
					{:else}
						<p class="text-2xs text-faint">End of log</p>
					{/if}
				</div>
			{/if}
		</div>
	</main>
</div>
