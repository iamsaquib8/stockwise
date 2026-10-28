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

## Educational, not advice

Every prompt starts with the same rules: explain what the data shows — signals,
trends, valuation context, risks and what to watch — in neutral, conditional
language, and never tell the reader to buy, sell, hold, accumulate or avoid
anything, never pick a winner, and give no price targets, entry or exit levels,
position sizes or allocations. This follows SEBI's rules for investment advisers
and research analysts (and its guidance on finfluencers): StockWise is neither.
Every AI section ends with a one-line disclaimer:

> AI-generated educational analysis, not investment advice or a recommendation
> to buy, sell or hold. StockWise is not a SEBI-registered investment adviser or
> research analyst.

A model can still ignore its instructions; treat its output as commentary. The
prompts live in `src/ai.rs` (`ai::prompts`), and a test checks that none of
them asks for a verdict or a call. When the prompts change, `PROMPT_VERSION` is
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
```

`ask` sends your question and the list of StockWise tools to the configured
model. The model picks tools, StockWise runs them (read-only, against live
data and your local portfolio), sends the results back, and the model answers
in plain English. The answer ends with the tools that ran. The tools are the
same ones the [MCP server](#tools) offers.

| Option | Default | Meaning |
|--------|---------|---------|
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

Answers are educational, not financial advice: the model is told to describe
what the data shows and its risks, not to tell you to buy or sell. **The tools
cannot place orders.**

**Privacy.** With a cloud provider, the question and every tool result go to
that provider — including your holdings, lots and tax figures when the model
calls `portfolio_summary`, `holdings_lots` or `tax_estimate`. Keep the default
local Ollama if that matters to you. The first request of each run prints the
usual notice.

## Use StockWise from AI assistants (MCP)

`stockwise mcp` is a [Model Context Protocol](https://modelcontextprotocol.io)
server built on the official Rust SDK. AI assistants start it and call its
tools; it runs on your machine with your data directory and needs no account.

```bash
stockwise mcp                          # stdio — what desktop and editor clients use
stockwise mcp --http 127.0.0.1:8788    # Streamable HTTP at http://127.0.0.1:8788/mcp
stockwise mcp --list-tools             # the tools and their JSON schemas
```

### Set it up

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

Every tool only reads. Each result is JSON with the data, a short "not
financial advice" `note`, and `truncated` when long lists were shortened.
Tools that take tickers accept `market` (`in`, `us`, `crypto`, `fx`,
`commodity`, `uk`, `de`, `jp`, `hk`, `sg`, `ca`, `au`).

| Tool | What it returns | Same as |
|------|-----------------|---------|
| `quote` | Price, change, day and 52-week range, volume, market cap, P/E for up to 20 tickers | `quote --json` |
| `history` | OHLCV bars (most recent `limit`, default 60) with the period's change, high and low | `history --json` |
| `technical` | SMAs, EMAs, RSI, MACD, Bollinger Bands, ATR, VWAP and a signal tally | `technical --json` |
| `risk` | Volatility, Sharpe, Sortino, Calmar, max drawdown, value at risk | `risk --json` |
| `search` | Tickers matching a company name | `search --json` |
| `markets` | Index levels; optional crypto, FX and global benchmarks | `markets --json` |
| `mf_search` / `mf_nav` / `mf_sip` | Mutual fund search, NAV with expense ratio, past SIP returns with XIRR (by AMFI code or ISIN) | `mf search/nav/sip --json` |
| `options_chain` / `options_pcr` / `options_maxpain` | Strikes around the money with OI, IV and greeks; put/call ratio; max pain | `options chain/pcr/maxpain --json` |
| `options_payoff` | A preset strategy's max profit/loss, breakevens, probability of profit, payoff table | `options payoff --json` |
| `screen` | A preset screen (undervalued, growth, dividend, momentum, bluechip) | `screen --json` |
| `backtest` | A built-in strategy on one ticker after costs, against buy and hold (last 10 trades) | `backtest --json` |
| `portfolio_summary` | Holdings, value, P&L, weights, XIRR and totals; unpriced holdings listed, never valued at 0 | `portfolio --json` |
| `holdings_lots` | Purchase lots with days held and short/long term, and recent sales | `portfolio lots` |
| `tax_estimate` | Realized and if-sold-today capital gains with India's set-off rules and ₹1.25 lakh exemption (or US/UK) | `tax --json` |
| `calendar` | Results, ex-dividend and split dates for tickers, or for your holdings and watchlist | `calendar --json` |
| `goal_sip` / `goal_retire` / `goal_education` | Goal calculators | `goal ... --json` |
| `alerts_list` | Your alerts checked against live data | `alert check --json` |
| `watchlist` | Your watchlist with live prices | `watch --json` |

Hosts see each tool's read-only and open-world hints; the portfolio tools are
marked as not reaching outside your machine.

### Resources and prompts

| Resource | Content |
|----------|---------|
| `stockwise://portfolio` | The portfolio as `portfolio --json` prints it |
| `stockwise://watchlist` | The watchlist with prices |
| `stockwise://alerts` | Alerts with their live status |

| Prompt | What it asks for |
|--------|------------------|
| `review_portfolio` (optional `focus`) | Value, P&L, concentration and risk of your holdings, without buy/sell calls |
| `tax_harvest_check` | Unrealized gains and losses by lot before 31 March, the LTCG exemption left and the set-off rules |
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

Put a TLS proxy in front of it before exposing it beyond your network.

### Registry and Claude Desktop extension

- [`server.json`](../server.json) describes the server for the
  [official MCP registry](https://registry.modelcontextprotocol.io) as
  `io.github.iamsaquib8/stockwise`, installable from crates.io, PyPI
  (`uvx stockwise mcp`) and GHCR.
- [`packaging/mcpb/`](../packaging/mcpb/README.md) builds a Claude Desktop
  extension (`.mcpb`) from a release binary.

### For agents: Agent Skill and llms.txt

- [`skills/stockwise/SKILL.md`](../skills/stockwise/SKILL.md) is an
  [Agent Skill](https://agentskills.io) that tells Claude Code and other agents
  how to use the CLI (`--json`, markets, safe commands) and the MCP server.
  Copy the folder into `~/.claude/skills/` (or a project's `.claude/skills/`).
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
| `OLLAMA_HOST` | `ollama_host` | `http://localhost:11434` | Ollama server (`host:port` or a full URL) |
| `OPENAI_API_KEY` | — | — | Key for `STOCKWISE_AI_PROVIDER=openai` |
| `ANTHROPIC_API_KEY` | — | — | Key for `STOCKWISE_AI_PROVIDER=anthropic` |
| `GEMINI_API_KEY` | — | — | Key for `STOCKWISE_AI_PROVIDER=gemini` |
| `STOCKWISE_AI` | `ai` | off | `1` to add AI sections to analysis commands by default (`--no-ai` turns it off for one run) |

Environment variables win over the [config file](commands.md#configuration-file).
API keys are read from the environment only, never from the config file.

```bash
STOCKWISE_MODEL=llama3.1:8b stockwise report portfolio
OLLAMA_HOST=192.168.1.20:11434 stockwise report TCS
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
  `ⓘ Sending this request's market data to <provider> (<model>)` on stderr.
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

Model output is commentary, not advice. See [DISCLAIMER.md](../DISCLAIMER.md).
