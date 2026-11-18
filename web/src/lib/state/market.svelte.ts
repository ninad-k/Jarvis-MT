import { api, type MarketJournalResponse, type MarketOrderType, type MarketViewDto, type PreviewDto } from '#lib/api.ts';
import { describeError, toApiError, type ApiError } from '#lib/errors.ts';
import { biasSentence, canPrepare, cleanSymbolInput, isExpired, validateRiskPercent, validateVolume } from '#lib/market.ts';

export type MarketPhase = 'idle' | 'loading' | 'ready' | 'error';
export type JournalPhase = 'idle' | 'loading' | 'ready' | 'error' | 'unsupported';
export type MarketTab = 'view' | 'briefing' | 'record';
/** How the Prepare dialog sizes the order: a lot size, or a risk % the server turns into lots. */
export type SizeMode = 'volume' | 'risk';

const CLOCK_MS = 10_000;
const JOURNAL_LIMIT = 15;

/** A server without the market endpoints answers 404 with no error body, which reads as `unknown`. */
const isMissing = (e: ApiError) => e.code === 'unknown';

function viewError(e: ApiError): string {
	if (isMissing(e)) return 'This server has no market view yet. Update the server to use it.';
	if (e.code === 'bridge_unavailable' || e.code === 'bridge_error')
		return `MT5 is not answering, so there are no prices to analyze. ${e.message}`;
	return describeError(e);
}

/**
 * The Market view panel: what to analyze, the latest view, the track record and the Prepare trade
 * dialog. A view is only read (`GET /api/market/view`) and never refreshes itself; "Prepare trade"
 * turns it into the usual preview, which the confirmation modal then handles.
 */
class MarketStore {
	tab = $state<MarketTab>('view');
	/** The symbol box. */
	symbolInput = $state('XAUUSD');
	view = $state<MarketViewDto | null>(null);
	phase = $state<MarketPhase>('idle');
	error = $state<string | null>(null);
	/** What the running request is for. */
	requested = $state<string | null>(null);
	/** Drives "expires in" and hides Prepare trade when a view runs out. Not a data refresh. */
	now = $state(Date.now());

	journal = $state<MarketJournalResponse | null>(null);
	journalPhase = $state<JournalPhase>('idle');
	journalError = $state<string | null>(null);

	// Prepare trade dialog
	prepareOpen = $state(false);
	/** The view the dialog was opened for, so a later analysis can't change what is being prepared. */
	prepareView = $state<MarketViewDto | null>(null);
	sizeMode = $state<SizeMode>('volume');
	volume = $state('');
	riskPercent = $state('');
	riskError = $state<string | null>(null);
	orderType = $state<MarketOrderType>('market');
	volumeError = $state<string | null>(null);
	prepareError = $state<string | null>(null);
	preparing = $state(false);

	private ctrl: AbortController | null = null;
	private journalCtrl: AbortController | null = null;

	get loading(): boolean {
		return this.phase === 'loading';
	}

	get expired(): boolean {
		return this.view !== null && isExpired(this.view.valid_until, this.now);
	}

	get canPrepare(): boolean {
		return this.view !== null && canPrepare(this.view, this.now);
	}

	/** Text for the screen-reader live region. */
	get announcement(): string {
		if (this.phase === 'loading') return `Analyzing ${this.requested ?? 'the symbol'}…`;
		if (this.phase === 'error') return `Analysis failed. ${this.error ?? ''}`;
		const v = this.view;
		if (this.phase !== 'ready' || !v) return '';
		const stand = v.stand_aside ? ` Stand aside: ${v.stand_aside_reason ?? 'no reason given'}.` : '';
		return `${v.symbol}: ${biasSentence(v.bias, v.confidence)}.${stand}`;
	}

	/** Keeps `now` current while the panel is on screen. Returns the stop function. */
	startClock(): () => void {
		const id = setInterval(() => (this.now = Date.now()), CLOCK_MS);
		return () => clearInterval(id);
	}

	/** Make `now` right at a moment that matters (the view's expiry). */
	tickAt(ms: number): () => void {
		const wait = Math.max(0, ms - Date.now()) + 50;
		// setTimeout caps at about 24 days; a view lasts hours, so no clamp is needed.
		const id = setTimeout(() => (this.now = Date.now()), wait);
		return () => clearTimeout(id);
	}

	async analyze(raw?: string): Promise<void> {
		const symbol = cleanSymbolInput(raw ?? this.symbolInput);
		if (!symbol) {
			this.phase = 'error';
			this.error = 'Enter a symbol to analyze.';
			return;
		}
		this.symbolInput = symbol;
		this.ctrl?.abort();
		const ctrl = new AbortController();
		this.ctrl = ctrl;
		this.phase = 'loading';
		this.error = null;
		this.requested = symbol;
		try {
			const next = await api.marketView(symbol, ctrl.signal);
			if (ctrl.signal.aborted) return;
			this.view = next;
			this.now = Date.now();
			this.phase = 'ready';
			// The view that was just made is now the newest journal entry.
			if (this.journalPhase === 'ready') void this.loadJournal();
		} catch (e) {
			const err = toApiError(e);
			if (err.code === 'aborted' || ctrl.signal.aborted) return;
			this.phase = 'error';
			this.error = viewError(err);
		}
	}

	async loadJournal(): Promise<void> {
		this.journalCtrl?.abort();
		const ctrl = new AbortController();
		this.journalCtrl = ctrl;
		if (this.journal === null) this.journalPhase = 'loading';
		try {
			const next = await api.marketJournal({ limit: JOURNAL_LIMIT }, ctrl.signal);
			if (ctrl.signal.aborted) return;
			this.journal = next;
			this.journalPhase = 'ready';
			this.journalError = null;
		} catch (e) {
			const err = toApiError(e);
			if (err.code === 'aborted' || ctrl.signal.aborted) return;
			if (isMissing(err)) {
				this.journalPhase = 'unsupported';
			} else if (this.journal === null) {
				this.journalPhase = 'error';
				this.journalError = describeError(err);
			} else {
				this.journalError = describeError(err);
			}
		}
	}

	setTab(tab: MarketTab): void {
		this.tab = tab;
		if (tab === 'record') void this.loadJournal();
	}

	// ---- Prepare trade ----

	openPrepare(): void {
		if (!this.view || !this.canPrepare) return;
		this.prepareView = this.view;
		// No default lot size, ever.
		this.volume = '';
		this.riskPercent = '';
		this.sizeMode = 'volume';
		this.orderType = 'market';
		this.volumeError = null;
		this.riskError = null;
		this.prepareError = null;
		this.preparing = false;
		this.prepareOpen = true;
	}

	closePrepare(): void {
		this.prepareOpen = false;
		this.prepareError = null;
		this.volumeError = null;
		this.riskError = null;
	}

	/**
	 * Check the volume, then ask the server for a preview. Returns it for the caller to hand to the
	 * confirmation modal, or null with the reason shown in the dialog (a limit, the kill switch...).
	 */
	async submitPrepare(maxRisk: number): Promise<PreviewDto | null> {
		const v = this.prepareView;
		if (!v || this.preparing) return null;
		this.prepareError = null;
		// Exactly one of the two sizes goes to the server; the other box is ignored.
		let size: { volume: number } | { risk_percent: number };
		if (this.sizeMode === 'risk') {
			const risk = validateRiskPercent(this.riskPercent, maxRisk);
			if (!risk.ok) {
				this.riskError = risk.error;
				return null;
			}
			this.riskError = null;
			size = { risk_percent: risk.value };
		} else {
			const check = validateVolume(this.volume);
			if (!check.ok) {
				this.volumeError = check.error;
				return null;
			}
			this.volumeError = null;
			size = { volume: check.value };
		}
		if (isExpired(v.valid_until, Date.now())) {
			this.prepareError = 'This view has expired. Close this and analyze again.';
			return null;
		}
		this.preparing = true;
		try {
			const dto = await api.marketPrepare({ view_id: v.id, order_type: this.orderType, ...size });
			return this.prepareOpen ? dto : null;
		} catch (e) {
			const err = toApiError(e);
			this.prepareError = isMissing(err) ? 'This server cannot prepare trades from a view yet.' : describeError(err);
			return null;
		} finally {
			this.preparing = false;
		}
	}
}

export const market = new MarketStore();
