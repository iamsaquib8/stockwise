# Brokers: live trading and holdings import

StockWise can read your holdings and place orders through a broker's API, and
import holdings from broker exports and CAS statements.

| Broker | `--broker` | Status | Holdings / positions / orders | MARKET, LIMIT | SL, SL-M | GTT |
|--------|-----------|--------|-------------------------------|---------------|----------|-----|
| Angel One SmartAPI | `angel` (default) | original integration | yes | yes | experimental | experimental |
| Zerodha Kite Connect | `zerodha` / `kite` | **experimental** | yes | yes | yes | single-leg |
| Upstox API v2/v3 | `upstox` | **experimental** | yes | yes | yes | not supported |
| Dhan API v2 | `dhan` | **experimental** | yes | yes | yes | single Forever Order |
| Fyers API v3 | `fyers` | **experimental** | yes | yes | yes | single-leg |

> **Experimental — untested against a live account.** The Zerodha, Upstox,
> Dhan and Fyers integrations, and stop-loss / GTT orders on Angel One, were
> built from each broker's public API documentation and tested against recorded
> and handwritten responses on a local mock server. Nobody has run them against
> a real account yet. Until you opt in, they only read (holdings, positions,
> orders) and preview (`--dry-run`); see [Turning on live orders](#turning-on-live-orders).
> Reports from real accounts are very welcome: run `stockwise trade selftest` and
> paste its report — see [Help verify a broker](#help-verify-a-broker).

Pick the broker per command with `--broker`, or set a default with
`STOCKWISE_BROKER=zerodha`. Without either, `trade` uses Angel One, as before.

```bash
stockwise trade holdings --broker zerodha
STOCKWISE_BROKER=dhan stockwise trade orders
```

## Order safety

Every order — any broker, any order type, GTT included — goes through the same
steps:

1. **Exact instrument.** The symbol must match one NSE cash-equity (`EQ`
   series) instrument in the broker's own instrument list: Angel One's
   `searchScrip`, Kite's instrument dump, Upstox's NSE instrument file, Dhan's
   NSE_EQ scrip master, Fyers' NSE_CM symbol master. Partial names, other
   series (`-BE`), ETFs with a similar name, and F&O contracts never match; two
   matches are refused.
2. **Price checks.** Prices must be positive with at most 2 decimals (never
   silently rounded). For stop-loss orders the trigger must sit on the right
   side of the market (above it for a BUY, below it for a SELL — otherwise it
   would fire at once), and the limit must be at or beyond the trigger in the
   order's direction. A limit more than 20% away from its trigger is treated as
   a typo. A GTT trigger must be at least 0.25% away from the last price, and a
   stop GTT (SELL below the market, BUY above it) needs a limit that is
   marketable at the trigger. Stop and GTT orders need a last price; if it
   can't be fetched the order is refused. Orders go to NSE only: a `.BO`
   symbol is refused rather than traded on NSE.
3. **Preview.** Instrument and broker token, side, quantity, order type,
   product (delivery / CNC), last price, estimated value.
4. **`max_order_value`.** If set in the broker's config file, any order whose
   estimated value is higher is refused (also on `--dry-run`).
5. **`--dry-run`** stops here: nothing is sent.
6. **Opt-in for experimental paths** (`"enable_orders": true`, below).
7. **Typed confirmation.** You type `yes`. Without a terminal (scripts, cron)
   the order is refused unless you pass `--yes`.

If the network fails while an order is in flight, StockWise says the order
state is **unknown** — check the broker app or `stockwise trade orders` before
retrying, so you don't place it twice. Cancelling a GTT goes through the same
preview, opt-in and confirmation.

The paper-trading daemon never sends broker orders.

## Order types

```bash
stockwise trade buy RELIANCE 10                              # MARKET
stockwise trade buy RELIANCE 10 --limit 1300                 # LIMIT
stockwise trade limit RELIANCE 10 1300                       # LIMIT buy (0.3 form)
stockwise trade sell RELIANCE 10 --trigger 1250              # SL-M: market sell once 1250 trades
stockwise trade sell RELIANCE 10 --trigger 1250 --limit 1245 # SL: limit sell at 1245 once 1250 trades
stockwise trade buy RELIANCE 10 --trigger 1400 --limit 1405  # SL buy (breakout entry)
```

All orders are delivery (CNC) on NSE with DAY validity. MARKET and SL-M orders
carry the broker's automatic market protection where the broker requires it
(Kite, Upstox).

### GTT (good till triggered)

A single-leg GTT places a LIMIT order when the trigger trades; it stays active
for up to a year.

```bash
stockwise trade gtt sell TCS 5 --trigger 3400 --limit 3390 --broker zerodha   # stop-loss below the market
stockwise trade gtt sell TCS 5 --trigger 4200 --limit 4190                    # target above it
stockwise trade gtt buy INFY 10 --trigger 1350 --limit 1355                   # buy the dip
stockwise trade gtt list
stockwise trade gtt cancel 112127
```

Two-leg (OCO) GTTs are not supported yet. Upstox GTT rules have no limit price,
so StockWise can't express "LIMIT at this price once triggered" there and
refuses GTTs for Upstox.

## Syncing trades and GTTs into the portfolio

`trade sync` reads the broker's trade book (executed fills) and turns delivery
trades into the lot-based portfolio: a buy becomes a lot with its fill date,
average fill price and order id; a sell comes out of the oldest lots (FIFO) and
is recorded as a realized sale for `tax`. It previews by default and writes only
with `--apply`.

```bash
stockwise trade sync --broker zerodha                 # preview: what would change
stockwise trade sync --broker zerodha --apply         # write it
stockwise trade sync --broker dhan --since 2026-10-01 --apply
stockwise trade sync --apply --notify --json          # for cron: JSON on stdout, notify on changes
```

- **Read-only at the broker.** Sync and `gtt list` run on a session that
  refuses any order, GTT or cancel request.
- **Idempotent.** Every fill applied is remembered (broker, order and trade id) in the
  portfolio file, and lots and sales keep the broker order id. Running sync
  twice, or after a partial fill completes, never counts a fill twice: the
  later part of a partly filled order becomes its own lot.
- **Delivery equity only.** Intraday (MIS / INTRADAY), margin (MTF), F&O and
  commodity fills are listed as skipped, not turned into lots. NSE and BSE
  fills of a stock you already hold go to that holding; a new holding takes
  the exchange of its fill (`.NS` or `.BO`).
- **Sells need the shares.** A sell larger than the shares the portfolio held
  on that date is left out (and not marked as synced) until you add the
  missing lots, e.g. with `portfolio add SYMBOL QTY PRICE --date`. Sells of a
  stock whose lots still wait for a split or bonus are held back too — run
  `portfolio actions SYMBOL --apply` first.
- **Dates.** Fills are dated by the broker's timestamp. Angel One gives a time
  only, so its fills are dated with the last NSE session (today once the
  market has opened, else the previous trading day).
- **Today only, mostly.** Angel One, Zerodha, Upstox and Fyers return today's
  trades only — run sync at the end of each trading day (or from cron). Dhan
  keeps a trade history, so `--since` fetches older fills there (up to 50
  pages; history rows get their NSE symbol from Dhan's scrip master). For
  older history on other brokers, `stockwise import` a tradebook export.
- **Open orders** (pending, partly filled) are listed; their fills sync once
  they appear in the trade book.
- **Quantity check.** For each symbol it touched, sync compares the
  portfolio's quantity with the broker's holdings list. Today's buys reach that
  list only after settlement (T+1), so a same-day gap is normal; a lasting gap
  means lots are missing or doubled. Use either `trade sync` (dated lots from
  fills) or `trade holdings` (quantities only, estimated dates) as your source
  for a broker — `trade holdings` matches quantities, so after a sync it only
  changes something when the two disagree.

### GTT status changes

`trade gtt list` remembers what it saw (in `gtt_seen_<broker>.json` in the
data directory) and shows what changed since the last look: GTTs that
triggered, were cancelled or expired, or no longer appear (brokers drop old
ones from the list). `--notify` sends those changes to your notification
channels (Telegram, Slack, Discord, email — see `stockwise notify status`);
only real state changes and vanished GTTs are sent, not a status spelling
change such as Angel One's `NEW` → `ACTIVE`. If every channel fails, the
record is not updated, so the changes are reported again next time.
`trade sync --apply` updates the same record and includes the changes;
`trade sync --apply --notify` notifies about them and about synced orders.

```bash
stockwise trade gtt list --broker zerodha --notify
stockwise trade gtt list --json     # {broker, gtts, changes, first_check, last_checked}
```

## Turning on live orders

Read-only commands work as soon as a broker is configured. To send real orders
through an experimental path, first compare a `--dry-run` preview with what the
broker app shows, then add to that broker's config file:

```json
"enable_orders": true
```

This applies to every order on Zerodha, Upstox, Dhan and Fyers, and to SL,
SL-M and GTT orders on Angel One. Angel One MARKET and LIMIT orders work as in
0.3 without it.

Most Indian brokers now accept API orders only from a **static IP registered
with your app** (Kite, Upstox, Dhan, Fyers and Angel One since April 2026).
Reads work from anywhere.

## Credentials and the OS keyring

`stockwise trade config --broker <name>` creates `<name>_config.json` in your
data directory (the path is printed; `stockwise doctor` shows it too) with
owner-only permissions (0600 on Unix). Fill in the placeholders and run the
command again.

On the next `trade` command run in a terminal (or `trade config` again),
secrets in that file — passwords, TOTP secrets, API secrets, access tokens —
**move into the OS keyring** when one is available
(macOS Keychain, Windows Credential Manager, the Secret Service on Linux
desktops), and the file is rewritten without them. It keeps the non-secret
settings plus `"secret_store": "keyring"` and the keyring entry name. Each data
directory (`STOCKWISE_HOME`) gets its own keyring entry.

- `stockwise trade config --keyring` moves them now and says why if it can't.
- Scripts and cron jobs (no terminal) never move secrets themselves; after a
  move they read them from the keyring, so on a machine where the keyring is
  locked when cron runs, keep `STOCKWISE_KEYRING=off`.
- With no keyring (headless Linux, containers, CI) the secrets stay in the
  owner-only file. Set `STOCKWISE_KEYRING=off` to always use the file.
- To change a secret later, put the new value in the file; it moves into the
  keyring again on the next command.
- `stockwise doctor` shows, per broker, where its secrets live.
- On macOS a rebuilt or upgraded binary may trigger a Keychain prompt the first
  time it reads an entry; choose "Always Allow". Unattended runs right after an
  upgrade may wait for that prompt.

Never pass credentials as command-line arguments — they end up in your shell
history. StockWise never prints or logs them, and sends them only to the
broker's own API.

Common optional settings in every broker config: `"max_order_value": 50000`
and `"enable_orders": true`.

## Angel One

1. Create an account at [angelone.in](https://www.angelone.in/) and an app at
   [smartapi.angelone.in](https://smartapi.angelone.in/); note its API key.
2. Enable TOTP on your account and note the base32 secret.
3. `stockwise trade config`, then fill in:

```json
{
  "api_key": "YOUR_API_KEY",
  "client_id": "YOUR_CLIENT_ID",
  "password": "YOUR_PASSWORD_OR_PIN",
  "totp_secret": "YOUR_TOTP_SECRET",
  "max_order_value": 50000
}
```

Angel One logs in with your PIN and a TOTP generated locally (built-in
HMAC-SHA1) on every command, so there is no `trade login` step. Existing
`angel_config.json` files keep working; their secrets move into the keyring on
first use.

## Zerodha Kite (experimental)

1. Create a Kite Connect app at [developers.kite.trade](https://developers.kite.trade/)
   (a paid Kite Connect plan); note the **API key** and **API secret**. Set the
   redirect URL to anything you can copy from the address bar, e.g.
   `http://127.0.0.1/`.
2. `stockwise trade config --broker zerodha` and fill in `api_key` and
   `api_secret`.
3. Every trading day: `stockwise trade login --broker zerodha`. It prints the
   Kite login URL; log in in your browser, then paste the address the browser
   was redirected to (it contains `request_token=…`). StockWise exchanges it for
   an access token (`checksum = sha256(api_key + request_token + api_secret)`)
   and saves it. Kite sessions end at 06:00 IST.

Holdings include T1 shares. Selling holdings without DDPI/POA may need a CDSL
TPIN authorisation in the Kite app first (Kite answers HTTP 428).

## Upstox (experimental)

1. Create an app at [account.upstox.com/developer/apps](https://account.upstox.com/developer/apps);
   note the **API key** and **API secret** and set a redirect URL such as
   `http://127.0.0.1/`.
2. `stockwise trade config --broker upstox`; fill in `api_key`, `api_secret`
   and the same `redirect_uri`.
3. Every trading day: `stockwise trade login --broker upstox` — log in in the
   browser, paste the redirect address (it contains `code=…`). Sessions end at
   03:30 IST. Orders use the v3 endpoint on `api-hft.upstox.com`.

## Dhan (experimental)

1. On [web.dhan.co](https://web.dhan.co) open My Profile → Access DhanHQ APIs
   and generate an **access token** (valid 24 hours).
2. `stockwise trade config --broker dhan`; put your Dhan **client id** in
   `api_key`.
3. `stockwise trade login --broker dhan` and paste the token (input is hidden).
   It is checked against your Dhan profile and saved (in the keyring when
   available). Repeat when it expires.

GTT orders on Dhan are single "Forever Orders".

## Fyers (experimental)

1. Create an app at [myapi.fyers.in/dashboard](https://myapi.fyers.in/dashboard);
   note the **App ID** (e.g. `XXXX-100`) and **Secret ID**, and set a redirect
   URL such as `http://127.0.0.1/`. Only App IDs ending in `-200` can place
   orders.
2. `stockwise trade config --broker fyers`; put the App ID in `api_key`, the
   Secret ID in `api_secret`, and the `redirect_uri`.
3. Every trading day: `stockwise trade login --broker fyers` — log in in the
   browser and paste the redirect address (it contains `auth_code=…`).
   StockWise validates it with `appIdHash = sha256(app_id:secret)`. Sessions end
   around 06:30 IST.

## Commands

```bash
stockwise trade setup [--broker B]          # How to connect
stockwise trade config [--broker B] [--keyring]
stockwise trade login --broker B            # Zerodha, Upstox, Fyers, Dhan
stockwise trade holdings                    # Live holdings (synced into `portfolio`)
stockwise trade positions                   # Open positions
stockwise trade orders                      # Today's order book
stockwise trade buy RELIANCE 10 --dry-run   # Resolve + preview, send nothing
stockwise trade sell TCS 5 --trigger 3400   # SL-M sell
stockwise trade gtt list [--notify]         # GTT orders + status changes since the last look
stockwise trade sync [--since DATE] [--apply]          # Fills into the portfolio (preview first)
stockwise trade selftest [--redact-amounts] [--json]   # Read-only check; report for a GitHub issue
```

## Help verify a broker

Zerodha, Upstox, Dhan and Fyers — and stop-loss / GTT orders on Angel One —
were built from public API docs and have never been run against a real account
by the maintainers. If you have an account with one of them, you can help take
the "experimental" label off in about five minutes, without placing an order.

1. Install the latest StockWise and set the broker up as described above
   (`trade setup`, `trade config`, `trade login`).
2. Run the self-test:

   ```bash
   stockwise trade selftest --broker zerodha
   stockwise trade selftest --broker zerodha --redact-amounts   # also hide prices
   stockwise trade selftest --broker zerodha --json             # machine-readable
   ```

   It is **strictly read-only**. It logs in (or loads today's session), reads
   the profile, holdings, positions, order book, trade book and GTT list,
   resolves RELIANCE, TCS and NIFTYBEES (the Nifty 50 ETF) exactly, runs a
   1-share LIMIT buy 5% below the market through every order check and stops
   before sending (like `--dry-run`), and compares the field names of each
   response with the documented ones. The session it uses refuses any order,
   GTT or cancel in code, and the self-test never calls those methods.
3. Read the report, then paste it into a
   [broker test report](https://github.com/iamsaquib8/stockwise/issues/new?template=broker_test_report.yml).
   It contains no credentials, tokens, names, e-mail addresses or holdings
   symbols; the account id is cut to its last two characters, and long numbers
   in error messages are masked. Field names, row counts, statuses and
   instrument tokens stay, as do the dry-run price and value unless you pass
   `--redact-amounts`. The command exits with status 1 if a check failed.
4. Optional, and only if you want to: place **one small order yourself** —
   say a 1-share LIMIT buy of a liquid stock below the market, which you can
   cancel — after adding `"enable_orders": true` to the broker's config. Check
   it in the broker's app, run `trade orders` and `trade sync`, and say in the
   report what happened. Nobody will ask you to place an order; this is the
   step that proves the write path.

### Verification status

| Broker | Status | Self-test reports | Small live order confirmed by a tester |
|--------|--------|-------------------|----------------------------------------|
| Angel One — MARKET, LIMIT | original integration (0.2), not experimental | reports welcome | — |
| Angel One — SL, SL-M, GTT | experimental | 0 of 3 | no |
| Zerodha Kite | experimental | 0 of 3 | no |
| Upstox | experimental | 0 of 3 | no |
| Dhan | experimental | 0 of 3 | no |
| Fyers | experimental | 0 of 3 | no |

**From experimental to verified.** A broker (or order path) loses the
experimental label in a release when all of these hold:

1. **3 independent self-test reports** — different testers and accounts — on
   a release that includes any fixes they led to, with no failed checks and no
   missing fields left unexplained.
2. **One confirmed small live order**, placed by a tester on their own
   initiative (never on request) through `trade buy` / `trade sell` (and, for
   GTT paths, `trade gtt`), that matched the preview in the broker's app and
   showed up in `trade orders` and `trade sync`.
3. A maintainer has reviewed the reports and updated this table, the
   `BrokerKind::experimental` flag and the CHANGELOG.

Until then, live orders on that path need `"enable_orders": true`, and every
preview says the path is experimental.

## Importing holdings

Import holdings from other brokers without manual entry:

```bash
stockwise import kite ~/Downloads/holdings.csv       # Zerodha Kite
stockwise import indmoney ~/Downloads/portfolio.csv  # INDmoney
stockwise import groww ~/Downloads/holdings.csv      # Groww (save the .xlsx as CSV)
stockwise import upstox ~/Downloads/holdings.csv     # Upstox
stockwise import cas ~/Downloads/cas.json            # CAMS / KFintech / NSDL / CDSL CAS via casparser
stockwise import csv ~/Downloads/stocks.csv          # Any CSV
```

Columns are detected by name (Symbol/Instrument/Stock Name/ISIN,
Quantity/Qty, Avg Cost/Average buy price), and leading account-detail rows are
skipped. A holdings export is a snapshot: existing lots and their dates are
kept and only the quantity follows the file, so re-importing never doubles a
holding. `trade holdings` syncs broker holdings the same way.

- **Groww** exports holdings as `.xlsx` with no ticker column: open it, save as
  CSV, and StockWise looks up each ISIN's NSE ticker (Yahoo search, one exact
  result only). Mutual-fund ISINs that don't trade on an exchange become
  `MF:<ISIN>`.
- **Upstox** holdings exports have no published layout; StockWise detects the
  usual columns (Symbol / Scrip Name / ISIN, Qty, Avg. Price). If it can't,
  use `import csv` with Symbol, Quantity and Average Price columns — and please
  open an issue with the header row.

### CAS (consolidated account statement)

CAS PDFs are password-protected and their tables don't survive plain text
extraction, so StockWise reads the output of the open-source
[casparser](https://github.com/codereverser/casparser) instead:

```bash
pip install casparser
casparser CAS.pdf -p <password> -o cas.json      # or -o cas.csv
stockwise import cas cas.json
```

- **CAMS / KFintech** (mutual funds): each scheme with units left becomes
  `MF:<AMFI code>` (or `MF:<ISIN>`). When the statement's transactions cover a
  scheme's whole balance (a CAS from the first purchase), every purchase becomes
  a dated lot (stamp duty included) and every redemption a FIFO sale, so `tax`
  and XIRR use the real dates. Otherwise the statement's cost value is used to
  sync the quantity; a statement that starts with units of unknown cost only
  updates funds you already track. The JSON and both CSV forms (transactions,
  `-s` summary) are supported. A scheme with lots from elsewhere is only rebuilt
  with `import cas FILE --replace`.
- **NSDL / CDSL** (demat): equities carry no purchase cost, so they only update
  the quantity of holdings already in your portfolio (keeping your cost);
  new ones are listed with a `portfolio add` hint. Mutual funds in a demat CAS
  are imported with their average cost when it is given.
- Feeding the PDF directly prints these instructions. Imported funds are
  valued at AMFI's latest end-of-day NAV in `portfolio`, `dashboard`, `wealth`,
  `rebalance`, `tax` and the rest, matched by AMFI code or by either ISIN
  (growth/payout or IDCW reinvestment). A scheme AMFI no longer lists stays
  N/A with a note, never 0 (see
  [Mutual funds and SGBs](commands.md#mutual-funds-and-sgbs)).
