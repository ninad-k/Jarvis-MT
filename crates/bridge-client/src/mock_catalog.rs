//! Symbol catalogs served by the mock EA's `list_symbols`.
use crate::protocol::{CatalogSymbol, SymbolTradeMode};

/// One table row: name, description, path, trade mode, visible, digits, base, profit currency.
type Row = (
    &'static str,
    &'static str,
    &'static str,
    SymbolTradeMode,
    bool,
    u32,
    &'static str,
    &'static str,
);

/// Forex pairs that exist twice: plain (hidden) and with the broker's `_` suffix (traded).
/// Columns: name, description, base, profit currency, digits.
const FX: [(&str, &str, &str, &str, u32); 10] = [
    ("EURUSD", "Euro vs US Dollar", "EUR", "USD", 5),
    (
        "GBPUSD",
        "Great Britain Pound vs US Dollar",
        "GBP",
        "USD",
        5,
    ),
    ("USDJPY", "US Dollar vs Japanese Yen", "USD", "JPY", 3),
    ("AUDUSD", "Australian Dollar vs US Dollar", "AUD", "USD", 5),
    ("USDCAD", "US Dollar vs Canadian Dollar", "USD", "CAD", 5),
    ("USDCHF", "US Dollar vs Swiss Franc", "USD", "CHF", 5),
    ("NZDUSD", "New Zealand Dollar vs US Dollar", "NZD", "USD", 5),
    ("EURGBP", "Euro vs Great Britain Pound", "EUR", "GBP", 5),
    ("EURJPY", "Euro vs Japanese Yen", "EUR", "JPY", 3),
    (
        "GBPJPY",
        "Great Britain Pound vs Japanese Yen",
        "GBP",
        "JPY",
        3,
    ),
];

// Paths use the backslash separator MT5 uses, so they exercise JSON escaping.
#[rustfmt::skip]
const ROWS: [Row; 35] = {
    use SymbolTradeMode::{CloseOnly, Disabled, Full};
    [
        ("XAUUSD_", "Gold vs US Dollar", "Metals\\XAUUSD_", Full, true, 2, "XAU", "USD"),
        ("XAGUSD_", "Silver vs US Dollar", "Metals\\XAGUSD_", Full, true, 3, "XAG", "USD"),
        ("US100_Spot", "Nasdaq 100 - Cash", "Indices\\US100_Spot", Full, true, 1, "USD", "USD"),
        ("US30_SPOT", "Dow Jones 30 - Cash", "Indices\\US30_SPOT", Full, true, 1, "USD", "USD"),
        ("US500_SPOT", "S&P 500 - Cash", "Indices\\US500_SPOT", Full, true, 1, "USD", "USD"),
        ("GER30_SPOT", "Germany 30 - Cash", "Indices\\GER30_SPOT", Full, true, 1, "EUR", "EUR"),
        ("UK100_SPOT", "UK 100 - Cash", "Indices\\UK100_SPOT", Full, true, 1, "GBP", "GBP"),
        ("JP225_SPOT", "Japan 225 - Cash", "Indices\\JP225_SPOT", Full, true, 1, "JPY", "JPY"),
        ("NAS1226", "Nasdaq 100 Dec 2026 future", "Futures\\NAS1226", Full, false, 1, "USD", "USD"),
        ("NAS0926", "Nasdaq 100 Sep 2026 future (expired)", "Futures\\NAS0926", CloseOnly, false, 1, "USD", "USD"),
        ("GC1226", "Gold Dec 2026 future", "Futures\\GC1226", Full, false, 1, "XAU", "USD"),
        ("DOWS1226", "Mini Dow Dec 2026 future", "Futures\\DOWS1226", Full, false, 0, "USD", "USD"),
        ("BTCUSD", "Bitcoin vs US Dollar", "Crypto\\BTCUSD", Full, true, 2, "BTC", "USD"),
        ("ETHUSD", "Ethereum vs US Dollar", "Crypto\\ETHUSD", Full, true, 2, "ETH", "USD"),
        ("LTCUSD", "Litecoin vs US Dollar", "Crypto\\LTCUSD", Full, false, 2, "LTC", "USD"),
        ("XRPUSD", "Ripple vs US Dollar", "Crypto\\XRPUSD", Full, false, 5, "XRP", "USD"),
        ("SOLUSD", "Solana vs US Dollar", "Crypto\\SOLUSD", Full, false, 3, "SOL", "USD"),
        ("WTI", "West Texas Intermediate crude oil", "Energies\\WTI", Full, true, 2, "USD", "USD"),
        ("BRENT", "Brent crude oil", "Energies\\BRENT", Full, true, 2, "USD", "USD"),
        ("NatGas_SPOT", "Natural Gas - Cash", "Energies\\NatGas_SPOT", Full, true, 3, "USD", "USD"),
        ("AAPL", "Apple Inc.", "Stocks\\US\\AAPL", Full, false, 2, "USD", "USD"),
        ("MSFT", "Microsoft Corporation", "Stocks\\US\\MSFT", Full, false, 2, "USD", "USD"),
        ("TSLA", "Tesla Inc.", "Stocks\\US\\TSLA", Full, false, 2, "USD", "USD"),
        ("NVDA", "NVIDIA Corporation", "Stocks\\US\\NVDA", Full, false, 2, "USD", "USD"),
        ("HK50_SPOT", "Hong Kong 50 - Cash", "Indices\\HK50_SPOT", Full, false, 1, "HKD", "HKD"),
        ("FRA40_SPOT", "France 40 - Cash", "Indices\\FRA40_SPOT", Full, false, 1, "EUR", "EUR"),
        ("AUS200_SPOT", "Australia 200 - Cash", "Indices\\AUS200_SPOT", Full, false, 1, "AUD", "AUD"),
        ("XPTUSD", "Platinum vs US Dollar", "Metals\\XPTUSD", Full, false, 2, "XPT", "USD"),
        ("XPDUSD", "Palladium vs US Dollar", "Metals\\XPDUSD", Full, false, 2, "XPD", "USD"),
        ("COPPER", "Copper - Cash", "Commodities\\COPPER", Full, false, 4, "USD", "USD"),
        ("SUGAR", "Sugar - Cash", "Commodities\\SUGAR", Full, false, 2, "USD", "USD"),
        ("COFFEE", "Coffee - Cash", "Commodities\\COFFEE", Full, false, 2, "USD", "USD"),
        ("USDX", "US Dollar Index", "Indices\\USDX", Full, false, 3, "USD", "USD"),
        ("VIX", "Volatility Index", "Indices\\VIX", Full, false, 2, "USD", "USD"),
        ("EURUSD.hist", "Euro vs US Dollar (history only)", "Archive\\EURUSD.hist", Disabled, false, 5, "EUR", "USD"),
    ]
};

/// The catalog the mock serves by default: a CFI-like CFD broker where the traded instruments
/// carry a trailing underscore (`EURUSD_`, `XAUUSD_`) and are shown in Market Watch, while a
/// plain, hidden `EURUSD` set exists next to them. Indices are `US100_Spot`/`*_SPOT`, dated
/// futures (`NAS1226`, `GC1226`, `DOWS1226`) are hidden, one expired future is close-only, and
/// there is crypto, energy and a few stocks. 55 symbols, in no particular order.
pub fn default_catalog() -> Vec<CatalogSymbol> {
    let mut catalog = Vec::with_capacity(FX.len() * 2 + ROWS.len());
    for (name, description, base, profit, digits) in FX {
        let entry = |name: String, path: String, visible: bool| CatalogSymbol {
            name,
            description: description.to_owned(),
            path,
            trade_mode: SymbolTradeMode::Full,
            visible,
            digits,
            currency_base: base.to_owned(),
            currency_profit: profit.to_owned(),
        };
        catalog.push(entry(name.to_owned(), format!("Forex\\{name}"), false));
        catalog.push(entry(
            format!("{name}_"),
            format!("FX Elite\\{name}_"),
            true,
        ));
    }
    for (name, description, path, trade_mode, visible, digits, base, profit) in ROWS {
        catalog.push(CatalogSymbol {
            name: name.to_owned(),
            description: description.to_owned(),
            path: path.to_owned(),
            trade_mode,
            visible,
            digits,
            currency_base: base.to_owned(),
            currency_profit: profit.to_owned(),
        });
    }
    catalog
}

/// A synthetic catalog of exactly `count` symbols (`SYM00000`, `SYM00001`, ...) in name order,
/// for paging and performance tests. Every 10th symbol is visible.
pub fn generated_catalog(count: usize) -> Vec<CatalogSymbol> {
    (0..count)
        .map(|i| CatalogSymbol {
            name: format!("SYM{i:05}"),
            description: format!("Synthetic instrument number {i}"),
            path: format!("Synthetic\\Group {}\\SYM{i:05}", i % 50),
            trade_mode: SymbolTradeMode::Full,
            visible: i % 10 == 0,
            digits: 2,
            currency_base: "USD".to_owned(),
            currency_profit: "USD".to_owned(),
        })
        .collect()
}
