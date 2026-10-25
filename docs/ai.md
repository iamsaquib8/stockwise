# AI Integration

StockWise can add plain-English commentary to its analysis. By default the
model runs **locally** through [Ollama](https://ollama.com): no stock data,
portfolio or credentials leave your machine. If you prefer a hosted model you
can opt in to OpenAI, Anthropic or Google Gemini instead (see
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

## Two ways to use it

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
