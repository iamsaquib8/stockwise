# Markets

`-m` picks the market bare tickers belong to. India (`in`, the default) and
the US (`us`) have their own lists and rules; crypto (`crypto`), currency pairs
(`fx`) and gold and silver (`commodity`) trade around the clock or 24x5; and
seven more stock exchanges come from one **market table** in `src/market.rs`.

## Global exchanges

| `-m` | Exchange | Suffix | Currency | Main index | Hours (local) | Holiday list bundled |
|------|----------|--------|----------|------------|---------------|----------------------|
| `uk` (`gb`, `lse`) | London Stock Exchange | `.L` | GBP, quoted in pence (GBp) | FTSE 100 | 08:00–16:30; 24/31 Dec close 12:30 | 2026, 2027 |
| `de` (`xetra`) | Deutsche Börse Xetra | `.DE` | EUR | DAX | 09:00–17:30 | 2026, 2027 |
| `jp` (`tse`) | Tokyo Stock Exchange | `.T` | JPY | Nikkei 225 | 09:00–11:30, 12:30–15:30 | 2026, 2027 |
| `hk` (`hkex`) | Hong Kong Exchanges | `.HK` (codes padded: `5` → `0005.HK`) | HKD | Hang Seng | 09:30–12:00, 13:00–16:00; eves close 12:00 | 2026, 2027 |
| `sg` (`sgx`) | Singapore Exchange | `.SI` | SGD | Straits Times | 09:00–12:00, 13:00–17:00; eves close 12:00 | 2026 (2027 provisional) |
| `ca` (`tsx`) | Toronto Stock Exchange | `.TO` | CAD | S&P/TSX Composite | 09:30–16:00 ET; 24 Dec closes 13:00 | 2026 |
| `au` (`asx`) | Australian Securities Exchange | `.AX` | AUD | S&P/ASX 200 | 10:00–16:00 Sydney; 24/31 Dec close 14:10 | 2026, 2027 |

```bash
stockwise -m uk quote VOD BARC          # VOD.L, BARC.L — prices in pence (118.20p)
stockwise -m jp history 7203 -p 1y      # Toyota, 7203.T
stockwise -m hk quote 700 5             # 0700.HK, 0005.HK
stockwise -m au sectors                 # S&P/ASX 200 sector indices
stockwise markets --global              # every exchange's main index, open or closed now
```

What each row holds:

- **Symbols**: the Yahoo Finance suffix bare tickers get. Symbols that already
  say where they trade (`TCS.NS`, `^FTSE`) pass through unchanged.
- **Currency**: prices use the local symbol (`£`, `€`, `¥`, `HK$`, `S$`, `C$`,
  `A$`). London quotes shares in pence: prices print as `118.20p`, market caps
  (which Yahoo reports in pounds) as `£27.38B`, and portfolio totals convert
  pence at GBP's rate ÷ 100. Enter London costs in pence too
  (`portfolio add VOD.L 100 118.2`). The bots and the simulator size positions
  in pounds, so they refuse `-m uk` for now.
- **Benchmarks** for `markets` (`-m uk markets` leads with the FTSE 100, FTSE
  250 and FTSE All-Share); the first one is the `markets --global` row.
- **Sectors** where Yahoo has a usable set: TOPIX-17 sector ETFs (Japan), Hang
  Seng sub-indices (Hong Kong), iShares S&P/TSX sector ETFs (Canada), S&P/ASX
  200 sector indices (Australia). The UK, Germany and Singapore have none, so
  `sectors`, `rotation` and `sectorcmp` say so.
- **Universe**: 14–15 large caps scanned by `movers`, `screen` and the bots.
- **Session and time zone**: European and Australian summer time are computed
  (no time-zone database needed); Tokyo, Hong Kong and Singapore have lunch
  breaks, during which `is_open` is false and alerts wait.
- **Holidays** (`src/clock.rs`): weekday closures and half days from each
  exchange's own calendar — LSE business days, Deutsche Börse's trading
  calendar, JPX's holiday page, SEHK circulars CT/075/25 and CT/077/26, ASX's
  trading calendar, TMX holiday notices, and for SGX the Ministry of Manpower
  public holidays it follows (2026 cross-checked against SGX's monthly
  trading-day counts; 2027 provisional until SGX publishes). Years outside a
  table fall back to "every weekday trades", and `markets --global` warns
  about it.
- **Tax**: `stockwise tax` has rules for India, the US and the UK only. Other
  listings are left out of an estimate and named.

## Commodities

`-m commodity` resolves `GOLD` (or `XAU`) to COMEX gold futures `GC=F` and
`SILVER` (`XAG`) to `SI=F`; other bare tickers are NSE listings, for gold and
silver ETFs (`GOLDBEES`) and Sovereign Gold Bonds. So `history gold`, `chart
silver`, `technical gold` and alerts work on them. `stockwise commodity` shows
spot in ₹/g, ₹/10 g and ₹/kg; see [commands.md](commands.md#commodities--bonds)
for where every number comes from — and what has no free source (MCX).

## Indian government bonds

`stockwise bonds` reads CCIL's tenor-wise indicative yields (RBI's benchmarks
as the fallback) and RBI's list of outstanding G-secs, and prices bonds with
`bonds calc`. `markets` shows the India 10-year yield. Details and limits are
in [commands.md](commands.md#commodities--bonds).

## Adding an exchange

1. Add a variant to `Exchange` and a `MarketInfo` row to `GLOBAL_MARKETS` in
   `src/market.rs`, in the same order (a test checks the order). Check every
   symbol against Yahoo Finance first.
2. Add its holiday table to `src/clock.rs` (weekday closures and half days,
   from the exchange's own calendar) and list the years in `official_years`.
   If its time zone has summer time not covered by a `TzRule`, add the rule
   with a test of the changeover instants.
3. Add its title to the `messages!` table in `src/i18n.rs` (`MarketXx`).
4. If the exchange quotes in a new currency, add its symbol to
   `currency_symbol` in `src/market.rs`.
5. `cargo test market clock` checks suffixes, universes, hours, that every
   holiday is a weekday in a covered year and that half days trade.

## Updating holiday tables

Exchanges publish next year's list between June (HKEX) and December (NSE,
TMX). Add the rows, move the year into `official_years`, and run `cargo test
clock`.
