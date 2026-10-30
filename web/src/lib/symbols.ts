/**
 * Pure helpers for the broker symbol mapping UI (docs/api.md "Broker symbol mapping"):
 * grouping by instrument class, source-badge wording, ranking the suggestions and the
 * canonical-versus-broker label shown in rows.
 */
import type { BrokerInfo, SymbolCandidate, SymbolMapping, SymbolSource } from './api';

// ---- Instrument classes -------------------------------------------------------

export type SymbolClassId = 'metal' | 'index' | 'energy' | 'crypto' | 'fx' | 'other';

/** Display order of the groups. */
export const CLASS_ORDER: readonly SymbolClassId[] = ['metal', 'index', 'energy', 'crypto', 'fx', 'other'];

export const CLASS_LABEL: Record<SymbolClassId, string> = {
	metal: 'Metals',
	index: 'Indices',
	energy: 'Energy',
	crypto: 'Crypto',
	fx: 'FX',
	other: 'Other'
};

const CLASS_ALIASES: Record<string, SymbolClassId> = {
	metal: 'metal',
	metals: 'metal',
	index: 'index',
	indices: 'index',
	indexes: 'index',
	energy: 'energy',
	energies: 'energy',
	crypto: 'crypto',
	cryptos: 'crypto',
	fx: 'fx',
	forex: 'fx',
	currency: 'fx',
	currencies: 'fx'
};

/** Map the server's `class` string onto a known group; anything else lands in "Other". */
export function normalizeClass(value: string | null | undefined): SymbolClassId {
	return CLASS_ALIASES[(value ?? '').trim().toLowerCase()] ?? 'other';
}

export interface MappingGroup {
	id: SymbolClassId;
	label: string;
	rows: SymbolMapping[];
}

/**
 * Group mappings by class in the fixed display order, leaving out empty groups. Inside a group the
 * server's order is kept (the config lists the main instruments first), so the table stays stable
 * while the user edits it.
 */
export function groupMappings(mappings: readonly SymbolMapping[]): MappingGroup[] {
	const buckets = new Map<SymbolClassId, SymbolMapping[]>();
	for (const m of mappings) {
		const id = normalizeClass(m.class);
		const list = buckets.get(id);
		if (list) list.push(m);
		else buckets.set(id, [m]);
	}
	return CLASS_ORDER.filter((id) => buckets.has(id)).map((id) => ({ id, label: CLASS_LABEL[id], rows: buckets.get(id)! }));
}

/** "US100 · Nasdaq, NAS100, USTEC" (just the name when there are no aliases). */
export function aliasLine(m: Pick<SymbolMapping, 'canonical' | 'aliases'>): string {
	return m.aliases.length ? `${m.canonical} · ${m.aliases.join(', ')}` : m.canonical;
}

/** Rows that need attention come first when the table is filtered to problems. */
export function needsAttention(m: Pick<SymbolMapping, 'available'>): boolean {
	return !m.available;
}

/** Instruments that need attention, each counted once (an unavailable row can also be listed
 *  as unmapped). */
export function attentionCount(mappings: readonly SymbolMapping[], unmapped: readonly string[]): number {
	const names = new Set(mappings.filter(needsAttention).map((m) => m.canonical));
	for (const u of unmapped) names.add(u);
	return names.size;
}

// ---- Source badge --------------------------------------------------------------

export interface SourceInfo {
	label: string;
	/** What the badge means, for the tooltip. */
	tip: string;
	/** Tailwind classes for the pill. */
	tone: string;
}

const SOURCES: Record<SymbolSource, SourceInfo> = {
	user: {
		label: 'user',
		tip: 'You chose this name yourself. It beats everything else and stays until you reset it.',
		tone: 'text-accent'
	},
	curated: {
		label: 'curated',
		tip: 'From the hand-checked list in config/broker_symbols.toml for this broker.',
		tone: 'text-up'
	},
	generated: {
		label: 'auto',
		tip: 'Found automatically by comparing the broker\'s symbol list with the instrument and its aliases. Worth a glance.',
		tone: 'text-ink'
	},
	pattern: {
		label: 'pattern',
		tip: 'Guessed from the broker\'s naming pattern (its usual suffix or prefix). Check it before trading.',
		tone: 'text-warn'
	},
	passthrough: {
		label: 'unchanged',
		tip: 'No mapping found, so the canonical name is sent to the broker as it is.',
		tone: 'text-mute'
	}
};

const UNKNOWN_SOURCE = (source: string): SourceInfo => ({
	label: source,
	tip: 'A source this version of the app does not know.',
	tone: 'text-mute'
});

export function sourceInfo(source: string): SourceInfo {
	return (SOURCES as Record<string, SourceInfo | undefined>)[source] ?? UNKNOWN_SOURCE(source);
}

// ---- Suggestions -----------------------------------------------------------------

/** Highest score first; ties keep the server's order. The current broker symbol is dropped. */
export function rankCandidates(candidates: readonly SymbolCandidate[], current?: string): SymbolCandidate[] {
	return candidates
		.map((c, i) => ({ c, i }))
		.filter(({ c }) => c.broker_symbol !== current)
		.sort((a, b) => b.c.score - a.c.score || a.i - b.i)
		.map(({ c }) => c);
}

/** Candidates that fit what the user has typed so far (all of them for an empty query). */
export function filterCandidates(candidates: readonly SymbolCandidate[], query: string): SymbolCandidate[] {
	const q = query.trim().toLowerCase();
	if (!q) return [...candidates];
	return candidates.filter((c) => c.broker_symbol.toLowerCase().includes(q) || c.description.toLowerCase().includes(q));
}

// ---- Labels -----------------------------------------------------------------------

export interface SymbolLabel {
	primary: string;
	/** The broker's own name, only when it differs from the canonical one. */
	secondary: string | null;
}

/** What a watchlist, position or order row shows: the canonical name, the broker's name beside it. */
export function symbolLabel(row: { symbol: string; canonical?: string | null }): SymbolLabel {
	const canonical = row.canonical?.trim();
	if (!canonical) return { primary: row.symbol, secondary: null };
	return { primary: canonical, secondary: canonical === row.symbol ? null : row.symbol };
}

export function brokerLine(b: { company: string; server: string }): string {
	return b.server ? `${b.company} · ${b.server}` : b.company;
}

/** "_" or "none" for the detected affixes. */
export function affixText(value: string | null | undefined): string {
	return value ? `"${value}"` : 'none';
}

export function fmtDetected(iso: string | null | undefined): string {
	if (!iso) return 'never';
	const d = new Date(iso);
	if (Number.isNaN(d.getTime())) return iso;
	return d.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

/** A file name for the exported map: `broker-symbols-cfi-financial-markets-llc.json`. */
export function exportFileName(broker: Pick<BrokerInfo, 'company'> | null): string {
	const slug = (broker?.company ?? '')
		.toLowerCase()
		.replace(/[^a-z0-9]+/g, '-')
		.replace(/^-+|-+$/g, '');
	return `broker-symbols${slug ? `-${slug}` : ''}.json`;
}

export function pluralize(n: number, one: string, many = `${one}s`): string {
	return `${n.toLocaleString('en-US')} ${n === 1 ? one : many}`;
}
