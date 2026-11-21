# NSE / BSE responses and test fixtures

The `nse`, `bse` and `exchange` data providers ([data-providers.md](data-providers.md#nse-and-bse))
read the JSON endpoints behind nseindia.com and bseindia.com. Those endpoints
are undocumented and change without notice, so StockWise pins down what it
expects in two places:

- **This page** lists every endpoint and the fields StockWise reads. Fields
  not listed are ignored, so new fields never break anything.
- **Fixtures** in [`src/providers/fixtures/`](../src/providers/fixtures/) hold
  one recorded-style response per endpoint. Parser tests run against them;
  no test touches the network.

## Fixture format

A fixture is the response body exactly as the endpoint sends it (JSON, or CSV
for the downloads), **trimmed and anonymised**:

- Keep the fields StockWise reads (below) and a few neighbours, so the
  fixture still looks like the real thing. Drop long arrays down to two or
  three entries.
- Replace real-time values with plausible made-up ones. Fixtures are test
  data, not market data: never commit a full live response (the exchanges'
  terms restrict redistributing their data), and never commit cookies,
  headers or anything tied to a session.
- Keep the quirks: numbers sent as strings (`"lowerCP": "1307.20"`, BSE's
  `"1,451.65"`), the misspelt `grapthData`, BSE's JSON-inside-a-string `Data`,
  NSE's IST-as-UTC chart stamps. Those are what the parsers must survive.
- Name it `<exchange>_<endpoint>.json` (or `.csv`) and load it with
  `include_str!` in the parser's tests.

The fixtures shipped with the first release of these providers are modelled
on the shapes these endpoints have long returned (and that open-source NSE/BSE
clients parse); they were not captured from a live session by the code in this
repository. When a live response differs, update the fixture and the parser
together and note the date here.

### Recording a fresh response

1. Open the page that shows the data in your browser — for example
   `https://www.nseindia.com/get-quotes/equity?symbol=RELIANCE` — with the
   developer tools' Network tab open, filtered to `api`.
2. Click the request (`quote-equity?symbol=RELIANCE`) and copy the response
   body.
3. Trim and anonymise it as above, save it under `src/providers/fixtures/`,
   and run `cargo test providers`.

Use your browser for this, as a person visiting the site would; don't script
requests around the exchanges' bot protection.

## NSE (`https://www.nseindia.com`)

Every API call follows one load of the home page (`/`) for session cookies.

| Endpoint | Fixture | Fields read |
|----------|---------|-------------|
| `/api/quote-equity?symbol=RELIANCE` | `nse_quote_equity.json` | `info.companyName`; `priceInfo.lastPrice`, `change`, `pChange`, `previousClose`, `open`, `intraDayHighLow.min`/`max`, `weekHighLow.min`/`max`, `lowerCP`/`upperCP` (strings). `{}` (or only `msg`/`info`) = unknown symbol; no `priceInfo` next to other keys = format change |
| `/api/quote-equity?symbol=RELIANCE&section=trade_info` | `nse_trade_info.json` | `marketDeptOrderBook.tradeInfo.totalTradedVolume` (lakh shares), `.totalMarketCap` (₹ crore); `securityWiseDP.deliveryToTradedQuantity` (%) |
| `/api/allIndices` | `nse_all_indices.json` | `data[]`: `indexSymbol` (or `index`), `last`, `variation`, `percentChange`, `open`, `high`, `low`, `previousClose`, `yearHigh`, `yearLow` |
| `/api/equity-stockIndices?index=NIFTY%2050` | `nse_equity_stock_indices.json` | `name`; `data[]`: `priority` (1 = the index's own row), `symbol`, `lastPrice`, `change`, `pChange`, `open`, `dayHigh`, `dayLow`, `previousClose`, `yearHigh`, `yearLow`, `totalTradedVolume` (shares), `meta.companyName` |
| `/api/chart-databyindex?index=RELIANCEEQN` (`?index=NIFTY%2050&indices=true` for an index) | `nse_chart_databyindex.json` | `grapthData`: `[epoch ms, price]` pairs, pre-open points tagged `"PO"`. The stamps are IST wall-clock time written as UTC; StockWise detects and corrects that |
| `/api/historical/cm/equity?symbol=RELIANCE&series=["EQ","BE"]&from=24-10-2024&to=24-10-2025` | `nse_historical_equity.json` | `data[]`: `CH_TIMESTAMP` (`2025-10-24`, or `mTIMESTAMP` `24-Oct-2025`), `CH_SERIES`, `CH_OPENING_PRICE`, `CH_TRADE_HIGH_PRICE`, `CH_TRADE_LOW_PRICE`, `CH_CLOSING_PRICE`, `CH_TOT_TRADED_QTY`. At most a year per request. On HTTP 404 StockWise tries `/api/historicalOR/generateSecurityWiseHistoricalData` (same fields) |
| `/api/historical/indicesHistory?indexType=NIFTY%2050&from=...&to=...` | `nse_indices_history.json` | `data.indexCloseOnlineRecords[]`: `EOD_TIMESTAMP` (`24-OCT-2025`), `EOD_OPEN_INDEX_VAL`, `EOD_HIGH_INDEX_VAL`, `EOD_LOW_INDEX_VAL`, `EOD_CLOSE_INDEX_VAL`; `data.indexTurnoverRecords[]`: `HIT_TIMESTAMP`, `HIT_TRADED_QTY` |
| `/api/search/autocomplete?q=reliance` | `nse_autocomplete.json` | `symbols[]`: `symbol`, `symbol_info`, `type` (only `equity` kept) |
| `https://nsearchives.nseindia.com/content/equities/EQUITY_L.csv` | inline in the tests | Columns `SYMBOL` and `NAME OF COMPANY`; search fallback, kept on disk for a day |

Prices of 0 (or `"-"`) mean "none" and are read as missing, never as ₹0.

## BSE (`https://api.bseindia.com/BseIndiaAPI/api`)

No cookies; calls carry `Referer` and `Origin` of `https://www.bseindia.com`.

| Endpoint | Fixture | Fields read |
|----------|---------|-------------|
| `ListofScripData/w?Group=&Scripcode=&industry=&segment=Equity&status=Active` | `bse_scrip_list.json` | A list (older answers: `{"Table": [...]}`) of `SCRIP_CD` (string or number), `scrip_id`, `Scrip_Name` (or `Issuer_Name`). Kept on disk for a day; maps `RELIANCE.BO` to `500325` and serves search |
| `getScripHeaderData/w?Debtflag=&scripcode=500325&seriesid=` | `bse_header.json` | `CurrRate.LTP`, `.Chg`, `.PcChg`; `Header.PrevClose`, `.Open`, `.High`, `.Low`, `.LTP`; `Cmpname.FullN`. Numbers are strings, sometimes with thousands separators. Neither `Header` nor `CurrRate` = format change |
| `StockReachGraph/w?scripcode=500325&flag=0&fromdate=&todate=&seriesid=` | `bse_stock_reach_graph.json` | `Data`: a JSON list sent as a string (or a list) of `{dttm: "Fri Oct 24 2025 09:15:00" (IST), vale1: price}` |
| `StockPriceCSVDownload/w?pageType=0&rbType=D&Scode=500325&FDates=24/10/2024&TDates=24/10/2025` | `bse_stock_prices.csv` | CSV columns found by name: `Date` (`24-October-2025`), `Open Price`, `High Price`, `Low Price`, `Close Price`, `No.of Shares` |
| `GetPublicIssue_par_updated/w?flag=1` | `tests/fixtures/bse_ipo_board.json` | IPO fallback (`src/ipo_fallback.rs`). `Table` rows: `Scrip_Name`, `Scrip_cd`, `IPO_NO`, `IR_flag` (only `IPO` is kept), `Status` (`L` open, `F` forthcoming), `Start_Dt`, `End_Dt` (ISO, `dd/mm/yyyy` or `19 Nov 2026`), `Price_Band`; the platform from `Exchange_Platform` / `TYPE` when present |
| `ipo_details_ng/w?stripono=<IPO_NO>` | `tests/fixtures/bse_ipo_detail.json` | `TableRows` of `Label` / `Label_flg` / `Value`: price band, minimum bid quantity, issue size (or separate fresh-issue and offer-for-sale rows), issue period, issue type, face value, lead managers, registrar; `Label_flg` `eXCHANGE_PLATFORM` gives mainboard or SME. Renamed to NSE's `issueInfo` titles and read by the same parser |
| `Pubissues_BBS_CumultveCatdem_ng/w?IPO_NO=<IPO_NO>` | `tests/fixtures/bse_ipo_demand.json` | Consolidated BSE + NSE category demand. `Table` rows: `CATEGORYNAME`, `BID_FOR` (shares offered), `QUANTITY` (shares bid), `PERC` (times subscribed). Categories are mapped to NSE's serials (QIB 1, FII 1(a), MF 1(c), NII 2, bNII 2.1, sNII 2.2, retail 3); anchor and market-maker rows are left out. No application counts |

The three IPO endpoints and their fields were read from the code of
bseindia.com's public-issue pages on 21 Nov 2026. BSE answered the app with
HTTP 403 from the machine they were written on, so the fixtures follow those
field names but were not captured live; update them from a browser session as
described above when BSE answers.
