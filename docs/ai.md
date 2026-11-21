# AI Integration

StockWise works with AI in three ways:

- **Commentary** — `report` and `--ai` add a plain-English section to analysis
  commands.
- **`stockwise ask`** — ask a question; the model calls StockWise's read-only
  tools (quotes, technicals, your portfolio, ...) and answers from the
  results. See [Ask in plain English](#ask-in-plain-english).
- **`stockwise mcp`** — an [MCP](https://modelcontextprotocol.io) server, so
  Claude Desktop, Claude Code, Cursor and other assistants can use the same
  tools. See [Use StockWise from AI assistants](#use-stockwise-from-ai-assistants-mcp).

By default the model runs **locally** through [Ollama](https://ollama.com): no
stock data, portfolio or credentials leave your machine. If you prefer a
hosted model you can opt in to OpenAI, Anthropic or Google Gemini instead (see
[Cloud providers](#cloud-providers-opt-in)). Everything else in StockWise
works without any AI.

## Setup (local, the default)

```bash
# Install Ollama (macOS; see ollama.com for Linux/Windows)
brew install ollama

# Start the server
ollama serve

# Pull the default model
ollama pull qwen3:14b

# Check that StockWise can see it
stockwise doctor
```

## Commentary: reports and `--ai`

**Dedicated reports** always use the model:

```bash
stockwise report intraday     # Notes on today's intraday scan
stockwise report longterm     # Research notes on the long-term scores
stockwise report portfolio    # Notes on your portfolio (funds at NAV, SGBs from NSE)
stockwise report RELIANCE     # Single-stock analysis
```

**Opt-in sections** on analysis commands — add `--ai` (or set `STOCKWISE_AI=1`
to make it the default):

```bash
stockwise analyze RELIANCE --ai
stockwise compare TCS INFY --ai
```

| Command | AI section |
|---------|-----------|
| `analyze` | Valuation in context, trend, risks, what to watch |
| `technical` | Whether the indicators agree, possible scenarios, levels to watch |
| `compare` | How the stocks differ on valuation, growth and risk (no overall pick) |
| `backtest` | How robust the results look, when the strategy works or fails |
| `screen` | Notes on the screened names, value-trap warnings |
| `deep` | Summary of the full report |

## Recommendations policy

StockWise shows rule-based actions: BUY, WATCH or AVOID for a stock you don't
hold, ADD, HOLD, TRIM or EXIT for one you do, and NO CALL when less than 60% of
the data is there. The actions are deterministic and come only from the rules
in `src/action.rs`, written out in
[methodology.md](methodology.md#actions-methodology-1). Each carries up to
three reasons, a confidence level, an invalidation price (BUY, ADD, HOLD, TRIM), a review date and
the disclaimer.

The AI explains; it never decides. Every prompt starts with the same rules:
explain what the data shows — signals, trends, valuation context, risks and
what to watch — and, when the command computed an action, explain that action
from its reasons. The model never invents an action, never upgrades or changes
the one computed, and never invents a price level (entry, stop, target or
invalidation). When the engine says NO CALL, the answer says there is not
enough data for a call. Every AI section ends with a one-line disclaimer:
AI-generated educational analysis; StockWise is not a SEBI-registered
investment adviser or research analyst.

A model can still ignore its instructions; trust the action line StockWise
printed over anything the model says. The prompts live in `src/ai.rs`
(`ai::prompts`), and a test checks that they carry these rules. When the prompts change, `PROMPT_VERSION` is
bumped: it is part of the cache key, so an answer cached under older wording is
never shown again.

AI is opt-in because a local model typically takes **30–60 seconds** per
answer, which would turn these sub-second commands into minute-long waits.
StockWise prints a progress line while the model works, and caches answers for
an hour when Redis is available.

The `daemon` workers write an end-of-day AI note when local Ollama is running.
With a cloud provider they only do so when started with `--ai`.

## Ask in plain English

```bash
stockwise ask "how is my portfolio doing?"
stockwise ask "compare TCS and INFY on RSI and one-year risk" --verbose
stockwise ask "what SIP reaches 1 crore in 15 years at 12%?"
stockwise ask "which of my holdings are long term for tax?" --json
stockwise ask -c "and which of them pay dividends?"   # carry on the last conversation
stockwise ask -i                                     # a conversation in the terminal
stockwise ask --playbook checkup                     # a prepared portfolio checkup
stockwise ask --auto "add TCS and INFY to my watchlist"
```

`ask` sends your question, your investor profile and the list of StockWise
tools to the configured model. The model picks tools, StockWise runs them
(against live data and your local files), sends the results back, and the
model answers in plain English. The answer ends with the tools that ran. The
tools are the same ones the [MCP server](#tools) offers.

**Your profile.** The system prompt carries your `[profile]`: mode, risk
level, horizon, the largest weight of one stock, the small-cap share and your
tax slab. The model uses it to judge concentration, and answers "does this
stock suit me?" with `fit_check`, which measures the stock against those
limits and your own notes. If you have not set a profile up, the model can
say so; `stockwise ask --auto "set up my profile"` walks through the
questions.

**Conversations.** Each answer is saved in `cache.db` (the newest 50
conversations; `cache clear` leaves them). `-c` carries on the newest one,
`--resume ID` a given one, `--sessions` lists them, and `-i` keeps asking
until you type `exit` or press Ctrl-D. A conversation belongs to the provider
and model it started with. Before each new question, long tool results from
earlier turns are trimmed; the answers built from them stay.

**Playbooks.** `--playbook` sends a prepared request that names its tools,
so the same task runs the same way each time. Add a question to extend it.

| Playbook | What it covers |
|----------|----------------|
| `checkup` | Value, P&L and XIRR; split by market, currency, cap tier and sector; holdings or small caps above your profile's limits; missing prices; results and dividend dates in the next 30 days; alerts close to triggering |
| `tax-season` | Realised gains and losses by term, the 112A exemption used and left, unrealised losses that could offset gains, advance-tax dates and amounts |
| `earnings` | Results dates for your holdings and watchlist in the next 30 days, and recent quarters against a year ago |

**Changes (`--auto`).** Without it the tools only read. With `--auto` the
model can also change your watchlist, alerts, goals, targets, notes and
profile, the [workspace tools](#changes-the-assistant-can-make). Each change
is recorded and `stockwise undo` reverses it. `ask` never records portfolio
lots and never places orders.

| Option | Default | Meaning |
|--------|---------|---------|
| `--interactive`, `-i` | off | Keep asking until you type `exit` (or press Ctrl-D) |
| `--continue`, `-c` | off | Carry on the newest saved conversation |
| `--resume ID` | — | Carry on conversation `ID` (see `--sessions`) |
| `--sessions` | off | List saved conversations |
| `--playbook NAME` | — | `checkup`, `tax-season` or `earnings` |
| `--auto` | off | Let the model change your workspace (undo with `stockwise undo`) |
| `--verbose`, `-v` | off | Print each tool call and the size of its result on stderr |
| `--json` | off | Print the transcript: question, answer, every tool call with its arguments, token usage |
| `--max-steps N` | 6 | Most model requests (1-20); the last one runs without tools, so the model must answer |
| `--max-tokens N` | 100000 | Token budget across requests (input + output); once used, the next request must answer |

At most 12 tool calls run per question, and each result is kept to about
24 KB (12 KB for local models): long price histories keep their newest bars
and option chains their strikes nearest the money. With Ollama the model
must answer once the conversation fills most of its 16k-token context.

**Models.** `ask` needs a model with tool (function) calling:

- Ollama: `qwen3` (the default `qwen3:14b`), `llama3.1`, `qwen2.5`,
  `mistral-nemo` and other models tagged *tools* on ollama.com. StockWise asks
  Ollama for a 16k-token context, because the tool definitions alone take a
  few thousand tokens. A model without tool support gets a clear error naming
  one that has it.
- Cloud (opt-in, see below): OpenAI (Responses API function calling),
  Anthropic (Messages API tool use) and Gemini (`generateContent` function
  calling). Running `ask` is the opt-in, as with `report`; `--ai` is not
  needed.

Answers are educational, not personalised advice: the model describes what the
data shows and its risks, and for an action it calls `stock_action` or
`portfolio_review` and explains what they return. It never makes up an action
or a price level. **The tools
cannot place orders.**

**Privacy.** With a cloud provider, the question and every tool result go to
that provider — including your holdings, lots and tax figures when the model
calls `portfolio_summary`, `holdings_lots` or `tax_estimate`. Keep the default
local Ollama if that matters to you. The first request of each run prints the
usual notice.

## Use StockWise from AI assistants (MCP)

> **Guide:** [Use StockWise from Claude, Gemini, Codex or Cursor](guide-mcp.md) is a short walkthrough of this section.

`stockwise mcp` is a [Model Context Protocol](https://modelcontextprotocol.io)
server built on the official Rust SDK. AI assistants start it and call its
tools; it runs on your machine with your data directory and needs no account.

```bash
stockwise mcp                          # stdio — what desktop and editor clients use
stockwise mcp --http 127.0.0.1:8788    # Streamable HTTP at http://127.0.0.1:8788/mcp
stockwise mcp --list-tools             # the tools and their JSON schemas
```

### Set it up

**One command, any app.** `stockwise connect` adds the server to every AI app
it finds on your computer, and installs the [agent skill](#for-agents-agent-skill-and-llmstxt)
where the app reads skills:

```bash
stockwise connect                          # every app found
stockwise connect antigravity gemini-cli   # just these
stockwise connect --all --allow-writes     # let them change your watchlist, alerts, goals, notes, profile
stockwise connect --list                   # the apps, found or not, and their config files
stockwise connect --dry-run                # what would change
stockwise connect cursor --remove
```

| App | `connect` name | Where it writes |
|-----|----------------|-----------------|
| Claude Desktop | `claude-desktop` | `claude_desktop_config.json` (`mcpServers`) |
| Claude Code | `claude-code` | `claude mcp add --scope user`, skill in `~/.claude/skills/` |
| Cursor | `cursor` | `~/.cursor/mcp.json` |
| Windsurf | `windsurf` | `~/.codeium/windsurf/mcp_config.json` |
| VS Code (Copilot agent mode) | `vscode` | the user `mcp.json` (`servers`) |
| Cline | `cline` | `cline_mcp_settings.json` |
| Gemini CLI | `gemini-cli` | `~/.gemini/settings.json` |
| Google Antigravity | `antigravity` | `~/.gemini/antigravity/mcp_config.json` (through its link, if it has one), skill in `~/.gemini/antigravity/skills/` |
| OpenAI Codex CLI | `codex` | `~/.codex/config.toml` (`[mcp_servers.stockwise]`) |
| Zed | `zed` | Zed's `settings.json` (`context_servers`) |
| LM Studio | `lm-studio` | `~/.lmstudio/mcp.json` |
| opencode | `opencode` | `~/.config/opencode/opencode.json` (`mcp`) |
| Goose, Continue | `goose`, `continue` | nothing: prints what to paste |

Only the `stockwise` entry changes; every other setting and server stays.
Each file is backed up to `<file>.stockwise.bak` first. A file `connect`
cannot read as plain JSON (Zed's comments, say) is left alone, with the
entry printed to paste by hand. The entry runs this `stockwise` by its full
path, so connect again after moving it. Restart the app afterwards. For
another user account on the same computer (a second Antigravity profile, say),
run `stockwise connect` while signed in as that user.

To set it up by hand instead:

**Claude Code**

```bash
claude mcp add stockwise -- stockwise mcp
claude mcp add stockwise -- stockwise mcp -m us     # US tickers by default
```

**Claude Desktop** — Settings → Developer → Edit Config, then restart:

```json
{
  "mcpServers": {
    "stockwise": {
      "command": "stockwise",
      "args": ["mcp"]
    }
  }
}
```

Use the full path from `which stockwise` (`where stockwise` on Windows) if
Claude Desktop can't find the command.

**Cursor** — `~/.cursor/mcp.json` (or `.cursor/mcp.json` in a project):

```json
{
  "mcpServers": {
    "stockwise": {
      "command": "stockwise",
      "args": ["mcp"]
    }
  }
}
```

**Without installing** — once the PyPI wheels are published, any client can
run it through [uv](https://docs.astral.sh/uv/): command `uvx`, args
`["stockwise", "mcp"]`. With Docker: command `docker`, args
`["run", "-i", "--rm", "-v", "stockwise:/data", "ghcr.io/iamsaquib8/stockwise", "mcp"]`.

**Other clients** — command `stockwise`, args `["mcp"]`, transport stdio; or
point an HTTP client at `stockwise mcp --http`.

Settings work as usual: `-m us` (or `STOCKWISE_MARKET`) sets the market for
bare tickers, `--portfolio NAME` picks a named portfolio, `STOCKWISE_HOME`
the data directory. Add them to `args` or the client's `env`.

### Tools

There are 71 tools that read, and 13 more that change your files when you
turn them on ([below](#changes-the-assistant-can-make)). Each result is JSON
with the data, a short "not financial advice" `note`, and `truncated` when long
lists were shortened.
Tools that take tickers accept `market` (`in`, `us`, `crypto`, `fx`,
`commodity`, `uk`, `de`, `jp`, `hk`, `sg`, `ca`, `au`).

| Tool | What it returns | Same as |
|------|-----------------|---------|
| `quote` | Price, change, day and 52-week range, volume, market cap, P/E for up to 25 tickers | `quote --json` |
| `history` | OHLCV bars (most recent `limit`, default 60) with the period's change, high and low | `history --json` |
| `technical` | SMAs, EMAs, RSI, MACD, Bollinger Bands, ATR, VWAP and a signal tally | `technical --json` |
| `risk` | Volatility, Sharpe, Sortino, Calmar, max drawdown, value at risk | `risk --json` |
| `verdict` | The 6-pillar scorecard: each pillar's 0–100 score, band and the facts behind it, the composite, and the data coverage. Pillars without data are marked as such. For the action, the model calls `stock_action` | `verdict --json` |
| `stock_action` | The rule-based action for one stock (`symbol`, optional `horizon`: `long` or `short`): kind, confidence, up to three reasons, invalidation price, review date, size hint, tax note, coverage and the disclaimer. NO CALL below 60% coverage | `verdict --json` (`action`) |
| `portfolio_review` | Each holding's action (optional `horizon`), sorted EXIT, TRIM, ADD, HOLD, then NO CALL, with reasons, invalidation, review date, size hint and tax notes | `portfolio review --json` |
| `scorecard` | Piotroski F-score and Altman Z-score with every criterion and its formula | `score --json` |
| `insights` | Positive, negative and neutral findings plus the shareholding split for one ticker | `insights --json` |
| `events` | Recent and upcoming corporate events with the price reaction around each | `events --json` |
| `quarterly_results` | Last 8+ quarters (or fiscal years) of results with YoY, QoQ, margin change, TTM, EPS vs consensus, bank NII and NPA | `results --json` |
| `search` | Tickers matching a company name | `search --json` |
| `markets` | Index levels; optional crypto, FX and global benchmarks | `markets --json` |
| `mf_search` / `mf_nav` / `mf_sip` | Mutual fund search, NAV with expense ratio, past SIP returns with XIRR (by AMFI code or ISIN) | `mf search/nav/sip --json` |
| `options_chain` / `options_pcr` / `options_maxpain` | Strikes around the money with OI, IV and greeks; put/call ratio; max pain | `options chain/pcr/maxpain --json` |
| `options_oi` | Open-interest walls and the build-up classification (long build-up, short covering, short build-up, long unwinding) | `options oi --json` |
| `options_payoff` | A preset strategy's max profit/loss, breakevens, probability of profit, payoff table | `options payoff --json` |
| `screen` | A preset screen (undervalued, growth, dividend, momentum, bluechip) | `screen --json` |
| `backtest` | A built-in strategy on one ticker after costs, against buy and hold (last 10 trades) | `backtest --json` |
| `portfolio_summary` | Holdings, value, P&L, weights, XIRR and totals; unpriced holdings listed, never valued at 0 | `portfolio --json` |
| `holdings_lots` | Purchase lots with days held and short/long term, and recent sales | `portfolio lots` |
| `tax_estimate` | Realized and if-sold-today capital gains with India's set-off rules and ₹1.25 lakh exemption (or US/UK) | `tax --json` |
| `tax_advance` | India's four advance-tax instalments from your realised gains, with Sec. 234C interest on a shortfall | `tax advance --json` |
| `harvest` | Lot-level loss-harvesting candidates and the unused ₹1.25 lakh LTCG exemption | `harvest --json` |
| `rebalance` | Trades to reach your saved target weights (or equal weight); buy-only when `inflow` is given | `rebalance --json` |
| `calendar` | Results, ex-dividend and split dates for tickers, or for your holdings and watchlist | `calendar --json` |
| `ipo_board` / `ipo_analyze` | Open, upcoming and recently listed Indian IPOs (NSE with BSE fallback) with the engine's verdict per issue; one issue's verdict, listing model, valuation, subscription, allotment odds and plan | `ipo --json` |
| `goal_sip` / `goal_education` | SIP for a goal: horizon-matched return, tax at redemption, outcome bands | `goal sip\|education --json` |
| `goal_retire` | Retirement corpus and SIP, withdrawal rate, survival after a 30% first-day fall, year-1 SWP vs FD tax (no outcome bands) | `goal retire --json` |
| `goal_swp` / `goal_prepay` | Withdrawal runway (SWP vs FD tax); loan prepayment against a SIP | `goal swp\|prepay --json` |
| `goal_list` | Your saved goals with live funded share | `goal list --json` |
| `alerts_list` | Your alerts checked against live data | `alert check --json` |
| `watchlist` | Your watchlist with live prices | `watch --json` |
| `profile` / `profile_questions` | Your investor profile and its guardrails; the onboarding questions | `profile show --json` |
| `portfolio_pme` / `portfolio_sell_preview` / `mf_bleed` / `income_calendar` | Index-twin comparison; what a sale would realise; fund cost drag; dividend calendar | `portfolio pme`, `portfolio income` |
| `portfolio_analyze` | The portfolio dashboard: by market, currency, cap tier and sector, tax terms, risk and observations | `portfolio analyze --json` |
| `portfolio_checkup` | Findings in six lenses (concentration, forensic flags, valuation, downside, fees and tax, goals) ranked by the money at stake, each with the command to inspect it | `portfolio checkup --json` |
| `market_conditions` | Trend, volatility, breadth and valuation with their history (readings, not a timing signal) | `conditions --json` |
| `picks` | Stocks ranked on value, quality, growth and trend among those scanned (the default universe, or `universe`, `cap`); finalists carry their rule-based `action` | `picks --json` |
| `movers` | Top gainers and losers today | `movers --json` |
| `dividends` | Dividend yield and income projection for one ticker | `dividends --json` |
| `goal_emergency` | Emergency reserve size against your liquid money, and the build-up plan | `goal emergency --json` |
| `fit_check` | Whether a stock fits **your own** rules: `yes`, `no` or `unclear`, with each rule passed, failed or unknown (weight of one stock and small caps against your profile, risk tier against your risk level, P/E and ROE limits from your note). It checks your rules; the action comes from `stock_action` | — |
| `changes_list` | The changes assistants made, newest first, with their undo command | `undo --list` |
| `picks_review` | How past `picks` lists did since each stock was first listed, against the benchmark's total return (`since`, `horizon`) | `picks review --json` |
| `verdicts` | The 6-pillar scores of up to 25 tickers in one call, compact, with `low_coverage` under 60% | `verdict --json`, per ticker |
| `bonds_yields` / `bonds_list` / `bonds_calc` | India's G-sec and T-bill yields by tenor; RBI's list of outstanding securities; price ↔ yield, accrued interest and duration | `bonds yields` / `bonds list` / `bonds calc` |
| `commodity` | Gold and silver spot, futures, ETFs and SGBs | `commodity --json` |
| `compare` | Stocks side by side on valuation, growth and risk | `compare --json` |
| `fx_history` | A currency against the rupee (or another base) over a period: change, high, low and recent daily rates | — |
| `returns` | Trailing price returns from 1 week to 5 years, annualized beyond a year | `returns --json` |
| `correlate` | Correlation of daily returns for 2 to 10 tickers over the last year | `correlate --json` |
| `portfolio_wrap` | The year in review (`year` or `fy`, optional `redact`) | `portfolio wrap --json` |
| `whats_hot` | Holdings and watchlist ranked by heat, then the market lists (`cap`, `limit`) | `hot --json` |
| `today` | Each holding's and watchlist name's day move, day P&L, σ, volume multiple and why (`days`) | `today --json` |
| `news_feed` | Merged headlines and filings for `symbols`, or for holdings and watchlist ranked by weight × importance | `news --json` |
| `monitor_status` | The holding monitor's thresholds, whether the worker runs it, and recent signals | `monitor status --json` |
| `digest` | Today's after-close digest: day P&L, contributors with reasons, monitor signals, alerts, dates coming up | `digest --json` |

Hosts see each tool's read-only and open-world hints; the portfolio tools are
marked as not reaching outside your machine.

### Changes the assistant can make

By default the server only reads. Two flags add tools that change your files:

```bash
stockwise mcp --allow-writes       # workspace: watchlist, alerts, goals, targets, notes, profile
stockwise mcp --allow-portfolio    # also record portfolio lots and sales (implies --allow-writes)
stockwise connect --allow-writes   # write the flag into every app's config
```

| Tier | Tools |
|------|-------|
| Workspace (`--allow-writes`) | `watchlist_add`, `watchlist_remove`, `alert_add`, `alert_remove`, `goal_save`, `goal_remove`, `target_set`, `target_remove`, `profile_set`, `profile_onboard`, `note_set` |
| Portfolio (`--allow-portfolio`) | `portfolio_add`, `portfolio_sell` (record a purchase lot or a FIFO sale; never an order). Two steps: without `confirm: true` they only return a preview (the lot, the holding after it, the realised gain), which the assistant shows you before recording |

- **Every change can be undone.** Before a change, the files it touches are
  copied to `undo/` in the data directory and the change is listed in
  `audit.json` (the newest 200). The tool's result carries its `change_id` and
  `stockwise undo N`. `stockwise undo` reverses the newest change,
  `stockwise undo --list` shows them all. Undo refuses when a later change or
  your own edit touched the same file; `--force` undoes anyway.
- **Profile onboarding.** `profile_questions` returns six plain questions
  (how closely you follow your investments, what you would do after a 20%
  fall, your horizon, experience, tax slab and regime). `profile_onboard`
  turns the answers into `[profile]` settings by a fixed mapping in StockWise,
  not by the model, and returns the reason for each. The `onboard_profile`
  prompt runs the whole conversation.
- **Notes.** `note_set` keeps your thesis for a stock and your own limits
  (largest weight, highest P/E, lowest ROE) in `notes.json`; `fit_check` reads
  them.
- Write tools are refused with `--allow-remote`: only a client on your own
  machine can change your files. Hosts see them as not read-only, and the
  `_remove` and portfolio tools as destructive, so most ask before each call.

### Resources and prompts

| Resource | Content |
|----------|---------|
| `stockwise://portfolio` | The portfolio as `portfolio --json` prints it |
| `stockwise://watchlist` | The watchlist with prices |
| `stockwise://alerts` | Alerts with their live status |
| `stockwise://profile` | The investor profile: mode, risk level, horizon and guardrails |
| `stockwise://picks/latest` | The newest logged picks list for each market and horizon |
| `stockwise://track-record` | How the last 90 days of picks did against the benchmark |
| `stockwise://methodology` | `docs/methodology.md`: how actions, verdicts and picks are computed |
| `stockwise://quote/{symbol}` | A quote for any ticker (a resource template) |

Over stdio, a host can subscribe to the portfolio, watchlist, alerts, profile,
picks and track-record resources and is told when the file changes.

| Prompt | What it asks for |
|--------|------------------|
| `daily_briefing` | Market conditions, the portfolio's day, triggered alerts and watchlist movers |
| `should_i_buy` (`symbol`, optional `amount`) | The stock's action, scores, coverage, risk and fit with your rules, without inventing a call |
| `exit_check` | Each holding's action, concentration and checkup findings |
| `smallcap_short_scan` (optional `market`) | `picks` with `cap` small and `horizon` short, with the track record and risks |
| `tax_aware_rebalance` | Drift from target weights and the tax each trim costs; `rebalance` with `raise` |
| `weekly_review` | The portfolio and past picks against the benchmark, with this week's changes |
| `mf_review` | Regular-plan cost against Direct, overlap with your stocks, recent NAVs (India) |
| `advance_tax_check` | Gains so far this financial year and the advance-tax instalments due (India) |
| `review_portfolio` (optional `focus`) | Value, P&L, concentration and risk of your holdings |
| `tax_harvest_check` | Unrealized gains and losses by lot before 31 March, the LTCG exemption left and the set-off rules |
| `onboard_profile` | Asks the onboarding questions one at a time and saves the profile (needs `--allow-writes`) |
| `explain_option_chain` (`symbol`, optional `expiry`) | OI, PCR, max pain, IV and greeks in plain English |

### Orders stay with you

The MCP server never places, modifies or cancels orders, and never reads
broker credentials. That is a deliberate choice: an assistant can misread a
request, and a prompt hidden in a web page or document it read could try to
trade for you.

`stockwise mcp --allow-orders` adds one more tool, `order_preview`. It checks
an order you are considering against the last price (the same trigger and
price checks as `trade`), estimates its value, and returns the
`stockwise trade` command — for example
`stockwise trade buy TCS 2 --broker zerodha --limit 4000` — for **you** to
review and run. That command shows its own preview, applies the
`max_order_value` cap from your broker config and asks you to type `yes`.
Nothing is sent to a broker from the MCP server.

### HTTP

`--http ADDR` serves [Streamable HTTP](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports#streamable-http)
at `http://ADDR/mcp`, stateless, with JSON responses. It follows the same
rules as `stockwise serve`:

- Loopback addresses only (`127.0.0.1:8788`, `localhost:8788`) unless you
  pass `--allow-remote`, which requires `STOCKWISE_API_TOKEN` (16+ characters);
  every request must then send `Authorization: Bearer <token>`.
- Without a token, requests must name a loopback host (`Host` header) and
  come from no browser origin or a `localhost` one, so a web page you visit
  can't reach it through DNS rebinding.

- Loopback alone does **not** authenticate operating-system users. On a
  shared machine, another local user can call the unauthenticated MCP HTTP
  endpoint and read available portfolio data. Set `STOCKWISE_API_TOKEN` even
  when listening only on `127.0.0.1`, and configure your HTTP MCP client to
  send `Authorization: Bearer <token>`.

Put a TLS proxy in front of it before exposing it beyond your network.

### Registry and Claude Desktop extension

- [`server.json`](../server.json) describes the server for the
  [official MCP registry](https://registry.modelcontextprotocol.io) as
  `io.github.iamsaquib8/stockwise`. It lists the crates.io package and the
  GHCR image (`ghcr.io/iamsaquib8/stockwise`); `uvx stockwise mcp` works once
  the PyPI wheels are published.
- [`packaging/mcpb/`](../packaging/mcpb/README.md) builds a Claude Desktop
  extension (`.mcpb`) from a release binary.

### For agents: Agent Skill and llms.txt

- [`skills/stockwise/SKILL.md`](../skills/stockwise/SKILL.md) is an
  [Agent Skill](https://agentskills.io) that tells Claude Code and other agents
  how to use the CLI (`--json`, markets, safe commands) and the MCP server: the
  tool tiers, undo, onboarding, `fit_check` and the common workflows.
  `stockwise connect` installs it for Claude Code and Antigravity; for other
  agents, copy the folder into their skills folder (a project's
  `.claude/skills/` or `.agents/skills/`).
- The documentation site publishes
  [`llms.txt`](https://iamsaquib8.github.io/stockwise/llms.txt) (an index of
  the docs and the cheatsheet's commands) and
  [`llms-full.txt`](https://iamsaquib8.github.io/stockwise/llms-full.txt)
  (every page in one file), and a Markdown copy of each page.

## Configuration

| Variable | Config key | Default | Purpose |
|----------|------------|---------|---------|
| `STOCKWISE_AI_PROVIDER` | `ai_provider` | `ollama` | `ollama`, `openai`, `anthropic` (or `claude`), `gemini` (or `google`) |
| `STOCKWISE_MODEL` | `model` | per provider | Model name; overrides the provider's default below |
| `OLLAMA_HOST` | `ollama_host` | `http://localhost:11434` | Ollama server (`host:port` or a full URL). A server that is not on this computer is refused over plain `http://` unless `STOCKWISE_ALLOW_REMOTE_OLLAMA=1` is set (use `https://` or a tunnel when you can); `ask` says when the server is remote |
| `STOCKWISE_ALLOW_REMOTE_OLLAMA` | — | off | `1` to allow plain `http://` to an Ollama server on another machine. Environment only |
| `OPENAI_API_KEY` | — | — | Key for `STOCKWISE_AI_PROVIDER=openai` |
| `ANTHROPIC_API_KEY` | — | — | Key for `STOCKWISE_AI_PROVIDER=anthropic` |
| `GEMINI_API_KEY` | — | — | Key for `STOCKWISE_AI_PROVIDER=gemini` |
| `STOCKWISE_AI` | `ai` | off | `1` to add AI sections to analysis commands by default (`--no-ai` turns it off for one run) |

Environment variables win over the [config file](commands.md#configuration-file).
API keys are read from the environment only, never from the config file.

```bash
STOCKWISE_MODEL=llama3.1:8b stockwise report portfolio
STOCKWISE_ALLOW_REMOTE_OLLAMA=1 OLLAMA_HOST=192.168.1.20:11434 stockwise report TCS
```

If the Ollama model isn't pulled, StockWise tells you the exact `ollama pull`
command instead of printing an empty section. An unknown
`STOCKWISE_AI_PROVIDER` value is an error, never a silent fallback: StockWise
won't guess where to send your data.

## Cloud providers (opt-in)

| Provider | `STOCKWISE_AI_PROVIDER` | Key | Default model | API |
|----------|-------------------------|-----|---------------|-----|
| OpenAI | `openai` | `OPENAI_API_KEY` | `gpt-6.1-sol` | Responses API (`/v1/responses`), `store: false` |
| Anthropic | `anthropic` | `ANTHROPIC_API_KEY` | `claude-opus-5-5` | Messages API (`/v1/messages`) |
| Google Gemini | `gemini` | `GEMINI_API_KEY` | `gemini-3.8-flash` | `generateContent` |

```bash
export STOCKWISE_AI_PROVIDER=anthropic
export ANTHROPIC_API_KEY=...            # from console.anthropic.com
stockwise doctor                        # shows provider and model, never the key
stockwise report RELIANCE               # first cloud request of the run prints a notice
```

To use a cheaper or newer model, set `STOCKWISE_MODEL` — for example
`gpt-6-luna`, `claude-sonnet-5-5`, `claude-haiku-5-5` or
`gemini-3.5-flash-lite`. `STOCKWISE_MODEL` applies to whichever provider is
active, so change or unset it when you switch providers.

The prompts are the same for every provider. On Claude models that support it
(Opus 5.5, Opus 5, Sonnet 5.5, Fable 5.x), StockWise asks the API to retry a
safety-classifier decline on Anthropic's recommended fallback model
(`fallbacks: "default"`); a request that is still declined is reported as an
error, not as an empty answer.

### Privacy

- **Nothing is sent unless you choose a cloud provider.** The default is
  Ollama on your own machine.
- **Choosing a provider is not enough on its own.** A cloud model is called
  only for `report` (which you run to get AI output) or when you add `--ai`
  (or set `STOCKWISE_AI=1`). Plain commands never contact it.
- **What is sent:** the prompt for that command — the market data StockWise
  fetched (prices, ratios, indicators, backtest numbers) and, for
  `report portfolio`, your holdings with quantities, cost and fund names. No broker
  credentials, API keys for other services, or data-directory files are
  included.
- **You are told when it happens:** the first cloud request of each run prints
  `ⓘ Sending this request's data to <provider> (<model>): market data, and your portfolio if a tool reads it` on stderr.
- Keys are read from the environment and sent only in request headers, never
  in URLs, logs or error messages. `doctor` names the variable it found, not
  its value.
- Requests to OpenAI set `store: false`. Each provider's own data-retention
  policy still applies; read it before sending portfolio data.
- Answers are cached for an hour in Redis (when running) under a hash of the
  prompt version, provider, model and prompt, so repeating a command doesn't
  send it again.

## Suggested local models

| Model | Download | Notes |
|-------|----------|-------|
| `qwen3:14b` | ~9 GB | Default. Strong structured analysis |
| `gemma3:12b` | ~8 GB | Good general reasoning |
| `phi4:14b` | ~9 GB | Good with financial metrics |
| `llama3.1:8b` | ~5 GB | Lighter and faster |

Model output is commentary on the data and on the rule-based action; it never makes the call. See [DISCLAIMER.md](../DISCLAIMER.md).
