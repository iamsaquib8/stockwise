# Use StockWise from Claude, Gemini, Codex or Cursor

This guide connects an AI assistant to StockWise so you can ask questions about
quotes, your portfolio and your tax estimate in plain English. It takes about
five minutes.

`stockwise mcp` is a [Model Context Protocol](https://modelcontextprotocol.io)
server. Your assistant starts it on your machine. The server reads your local
StockWise data and answers with JSON. It needs no account and no API key.

You need: StockWise installed (see [Install](install.md)) and an MCP client:
Claude Desktop or Claude Code, OpenAI Codex, Gemini CLI, Google Antigravity,
Cursor, Windsurf, VS Code, Cline, Zed, LM Studio, opencode, Goose, Continue
or another.

## What the assistant can and cannot do

- **It can read.** The read tools cover quotes (up to 25 tickers a call),
  history, technicals, risk, scorecards and 6-pillar scores (one stock with
  `verdict`, up to 25 with `verdicts`), events, mutual funds, option chains,
  IPOs, screens, picks and their track record, market conditions, backtests,
  your portfolio dashboard, lots and year in review, tax estimate, advance
  tax, harvest and rebalance plans, calendar, goals, watchlist and alerts.
  For Indian investors there are also G-sec yields and a bond calculator,
  gold, silver and Sovereign Gold Bonds in rupees, USD/INR history, trailing
  returns, correlation and side-by-side comparison.
- **It follows a recommendations policy.** Buy or sell actions come only from
  StockWise's own rule-based engine, never from the assistant; the assistant
  explains them and never invents price levels. When less than 60% of the data
  behind a score is there, it says there is not enough data for a call. Every
  answer keeps the "educational, not advice" note. (The action engine itself,
  `stock_action` and `portfolio_review`, arrives in a later 0.9 build; until
  then the assistant describes scores and readings only.)
- **It can check a stock against your own rules.** Ask "does TCS fit my
  rules?" and `fit_check` answers yes, no or unclear from your profile's
  limits and your notes, rule by rule. It is not a recommendation: it never
  says buy or sell.
- **It cannot place orders.** The server never places, changes or cancels an
  order and never reads broker credentials. The optional `--allow-orders` flag
  adds one tool that only checks an order and returns a command such as
  `stockwise trade buy TCS 2 --limit 4000` for you to review and run yourself.
- **It changes your data only if you allow it.** By default every tool only
  reads. With `--allow-writes` it can also set up your profile and change your
  watchlist, alerts, goals, targets and notes; with `--allow-portfolio` it can
  record purchase lots and sales. Each change is journalled, and
  `stockwise undo` reverses it.

One point on privacy. The server runs on your machine, but a hosted assistant
sends what its tools return to its own model provider. If you ask about your
portfolio, the holdings go with the question. If that is a problem, use
`stockwise ask "question"`, which can run a local model through Ollama. See [AI](ai.md).

## 1. Connect your client

The quickest way is one command. It finds the AI apps on your computer and
adds StockWise to each:

```bash
stockwise connect --list          # the apps it knows, and which it found
stockwise connect                 # connect every app it found
stockwise connect antigravity     # or name the apps
```

It edits only the `stockwise` entry of each app's config, keeps a backup next
to it (`<file>.stockwise.bak`), and installs the StockWise skill for Claude
Code and Antigravity. Add `--allow-writes` to let the assistant change your
watchlist, alerts, goals and profile. Restart each app afterwards.
`stockwise connect cursor --remove` takes it out again.

Google Antigravity keeps its servers in `~/.gemini/antigravity/mcp_config.json`
(on some installs a link to `~/.gemini/config/mcp_config.json`; `connect`
writes through the link). Each user account has its own, so for a second
profile on the same computer, run `stockwise connect antigravity` signed in
as that user.

To edit the configs by hand instead:

Claude Code:

```bash
claude mcp add stockwise -- stockwise mcp
claude mcp add stockwise -- stockwise mcp -m us    # US tickers by default
```

Claude Desktop: open Settings, then Developer, then Edit Config. Add this and
restart the app.

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

Cursor, Windsurf, Gemini CLI, Antigravity, LM Studio and Cline use the same
`mcpServers` JSON, in `~/.cursor/mcp.json`, `~/.codeium/windsurf/mcp_config.json`,
`~/.gemini/settings.json`, `~/.gemini/antigravity/mcp_config.json`,
`~/.lmstudio/mcp.json` and Cline's MCP settings. VS Code uses `"servers"`
with `"type": "stdio"`; Zed `"context_servers"`; opencode `"mcp"` with
`"type": "local"` and the command as a list.

OpenAI Codex, in `~/.codex/config.toml`:

```toml
[mcp_servers.stockwise]
command = "stockwise"
args = ["mcp"]
```

If the client cannot find the command, use the full path from `which
stockwise` (`where stockwise` on Windows) as `command`.

Other settings go in `args` or in the client's `env`:

| Setting | How |
|---|---|
| Market for bare tickers | `"args": ["mcp", "-m", "us"]` or `STOCKWISE_MARKET` |
| A named portfolio | `"args": ["mcp", "--portfolio", "NAME"]` |
| Data folder | `STOCKWISE_HOME` |

## 2. Check that it works

List the tools from your terminal. This does not need the assistant.

```bash
stockwise mcp --list-tools
```

It prints a JSON array. Each entry has a `name`, a `description` and an
`inputSchema`. Then ask your assistant something that needs data:

> What is the current price and P/E of TCS and INFY?

The assistant shows a tool call to `quote` and then answers from the result.

## 3. Ask useful questions

Questions that work well, grouped by what they use:

| Ask | Tools used |
|---|---|
| "Summarise my portfolio: value, P&L, XIRR and my biggest holdings." | `portfolio_summary` |
| "Which of my holdings are long-term and which are short-term?" | `holdings_lots` |
| "What is my capital-gains tax so far this year?" | `tax_estimate` |
| "Do I owe advance tax by the next due date?" | `tax_advance` |
| "Which lots would 20 shares of TCS sell, and what is the tax?" | `portfolio_sell_preview` (never records the sale) |
| "What do my Regular-plan funds cost me against Direct?" | `mf_bleed` |
| "What could I book before 31 March, and what is my unused LTCG exemption?" | `harvest` |
| "If I invest another ₹50,000, how should it be split to match my targets?" | `rebalance` |
| "How risky has HDFCBANK been over the last year?" | `risk` |
| "Explain the NIFTY option chain for this expiry in plain English." | `options_chain`, `options_oi` |
| "Show my portfolio by market, sector and cap tier, and what stands out." | `portfolio_analyze` |
| "Is the market expensive or stretched right now?" | `market_conditions` (readings, not a timing call) |
| "Does INFY fit my rules if I put ₹50,000 in?" | `fit_check` |
| "Which NIFTY 50 stocks score best on value and quality?" | `picks` |
| "Rank small caps for the next few weeks." | `picks` with `cap: small`, `horizon: short` |
| "How did the picks lists of the last 3 months do against NIFTY?" | `picks_review` |
| "Score TCS, INFY, HCLTECH, WIPRO and TECHM side by side." | `verdicts` |
| "Compare HDFCBANK, ICICIBANK and KOTAKBANK on valuation and ROE." | `compare` |
| "What have NIFTYBEES' returns been over 1, 3 and 5 years?" | `returns` |
| "Do my bank stocks move together?" | `correlate` |
| "Where are G-sec yields, and what does 7.18% GS 2033 yield at ₹101.50?" | `bonds_yields`, `bonds_calc` |
| "Which Sovereign Gold Bond trades at the biggest discount to gold?" | `commodity` with `view: sgb` |
| "How much has the dollar moved against the rupee this year?" | `fx_history` |
| "How did my portfolio do in FY 2025-26?" | `portfolio_wrap` with `fy: 2025-26` |

With `--allow-writes`:

| Ask | Tools used |
|---|---|
| "Set up my investor profile." | `profile_questions`, then `profile_onboard` |
| "Add TCS and INFY to my watchlist and alert me if TCS falls below 3,800." | `watchlist_add`, `alert_add` |
| "Save a goal: 25 lakh for a house by June 2030." | `goal_save` |
| "Note my thesis on HDFCBANK, with a P/E limit of 22." | `note_set` |
| "What did you change today?" | `changes_list` |

Ready-made prompts are built in. In clients that support MCP prompts, pick one
from the prompt menu:

| Prompt | What it does |
|---|---|
| `daily_briefing` | Market conditions, your portfolio's day, triggered alerts and watchlist moves |
| `should_i_buy` | A stock's scores, coverage, risk and fit with your rules (argument: `symbol`, optional `amount`) |
| `exit_check` | Each holding's scores and coverage, concentration and checkup findings |
| `smallcap_short_scan` | Small caps ranked for weeks to months, with the short-horizon track record |
| `tax_aware_rebalance` | Drift from your targets and the tax each trim would cost |
| `weekly_review` | The portfolio against its benchmark, past picks lists, and this week's changes |
| `mf_review` | Regular-plan cost against Direct, fund overlap with your stocks (India) |
| `advance_tax_check` | Capital gains this financial year and the advance-tax instalments due (India) |
| `review_portfolio` | Value, P&L, concentration and risk |
| `tax_harvest_check` | Unrealised gains and losses by lot before 31 March |
| `explain_option_chain` | An option chain in plain words |
| `onboard_profile` | A few plain questions, then your profile is saved (needs `--allow-writes`) |

Where a prompt asks for a ticker, clients that support completion suggest
your holdings and watchlist first, then search results.

The server also offers resources:

| Resource | What it holds |
|---|---|
| `stockwise://portfolio`, `stockwise://watchlist`, `stockwise://alerts`, `stockwise://profile` | Your data, live-priced |
| `stockwise://picks/latest` | The newest logged picks list for each market and horizon |
| `stockwise://track-record` | How the last 90 days of picks did against the index |
| `stockwise://methodology` | How picks and verdicts are scored |
| `stockwise://quote/{symbol}` | A quote for any ticker |

Over stdio a client can subscribe to the portfolio, watchlist, alerts,
profile and picks resources; the server then sends
`notifications/resources/updated` when their files change (checked every two
seconds), for example when the background worker marks an alert triggered.

### For client authors

- Every tool has `readOnlyHint`, `destructiveHint`, `idempotentHint` and
  `openWorldHint`. Read tools are read-only; write tools touch only local
  files (closed-world), and the ones that remove or replace saved data are
  marked destructive.
- `quote`, `history`, `verdict`, `verdicts`, `picks`, `picks_review`,
  `portfolio_summary`, `portfolio_analyze`, `portfolio_checkup`, `rebalance`,
  `tax_estimate` and `market_conditions` publish an `outputSchema`. Their
  `structuredContent` follows it (an envelope of `data`, `note` and, when
  lists were shortened, `truncated`); the same JSON is also sent as text for
  older clients.
- A read tool stops after 60 seconds (180 for market scans and portfolio
  analyses). Set `STOCKWISE_MCP_TIMEOUT=SECONDS` in the client's `env` to
  change it. Writes are never cut off.

## 4. Check the answer

An assistant can misread a number. Keep these habits:

- Open the tool call and look at the returned values for anything that matters.
- Treat tax figures as estimates. Compare them with your broker's tax P&L.
- Ask for the source of a number. Every result carries a short "not financial
  advice" note, and long lists say when they were truncated.

## 5. Over HTTP, or without installing

```bash
stockwise mcp --http 127.0.0.1:8788    # Streamable HTTP at http://127.0.0.1:8788/mcp
```

The server accepts loopback addresses only, unless you pass `--allow-remote`
and set `STOCKWISE_API_TOKEN`. Use HTTP only with clients that need it. The
[AI page](ai.md#http) lists the safety rules. For Docker, `uvx` and the registry
entry, see [AI](ai.md#registry-and-claude-desktop-extension).

## Next steps

- [Import your trades](guide-portfolio.md). The assistant has more to read once
  your portfolio is in.
- [Script it with JSON, CSV and jq](guide-scripting.md) for the same data
  without an assistant.
- The full tool table, the resources and the safety model are in
  [AI](ai.md#use-stockwise-from-ai-assistants-mcp). Agents can also read the
  [llms.txt](https://iamsaquib8.github.io/stockwise/llms.txt) file.
- `stockwise ask "question"` runs the same tools from the terminal with a local
  model. See [Ask in plain English](ai.md#ask-in-plain-english).
- Server does not start? See [Troubleshooting](troubleshooting.md).
