# Use StockWise from Claude or Cursor

This guide connects an AI assistant to StockWise so you can ask questions about
quotes, your portfolio and your tax estimate in plain English. It takes about
five minutes.

`stockwise mcp` is a [Model Context Protocol](https://modelcontextprotocol.io)
server. Your assistant starts it on your machine. The server reads your local
StockWise data and answers with JSON. It needs no account and no API key.

You need: StockWise installed (see [Install](install.md)) and an MCP client:
Claude Code, Claude Desktop, Cursor or another.

## What the assistant can and cannot do

- **It can read.** The 34 tools cover quotes, history, technicals, risk,
  scorecards, events, mutual funds, option chains, IPOs, screens, backtests,
  your portfolio and lots, tax estimate, advance tax, harvest and rebalance
  plans, calendar, watchlist and alerts.
- **It cannot place orders.** The server never places, changes or cancels an
  order and never reads broker credentials. The optional `--allow-orders` flag
  adds one tool that only checks an order and returns a command such as
  `stockwise trade buy TCS 2 --limit 4000` for you to review and run yourself.
- **It cannot change your data.** Every tool only reads.

One point on privacy. The server runs on your machine, but a hosted assistant
sends what its tools return to its own model provider. If you ask about your
portfolio, the holdings go with the question. If that is a problem, use
`stockwise ask "question"`, which can run a local model through Ollama. See [AI](ai.md).

## 1. Connect your client

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

Cursor: put the same JSON in `~/.cursor/mcp.json`, or in `.cursor/mcp.json`
inside a project.

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

Ready-made prompts are built in. In clients that support MCP prompts, pick one
from the prompt menu:

- `review_portfolio` for value, P&L, concentration and risk, with no buy or
  sell calls.
- `tax_harvest_check` for unrealised gains and losses by lot before 31 March.
- `explain_option_chain` for an option chain in plain words.

The server also offers three resources: `stockwise://portfolio`,
`stockwise://watchlist` and `stockwise://alerts`.

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
