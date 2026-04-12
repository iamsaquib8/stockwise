# AI Integration (Ollama)

StockWise uses local AI via [Ollama](https://ollama.com) for intelligent analysis. All inference runs on your machine — no data sent to external AI services.

## Setup

```bash
# Install Ollama (if not already)
brew install ollama

# Start the server
ollama serve

# Pull the default model
ollama pull gemma3:12b
```

## How It Works

When Ollama is running, AI analysis is **automatically appended** to these commands:

| Command | AI Enhancement |
|---------|---------------|
| `analyze` | Fundamental analysis verdict (buy/hold/sell) |
| `technical` | Multi-indicator confluence interpretation |
| `compare` | "Which stock is better?" verdict |
| `backtest` | Strategy viability assessment |
| `screen` | Top picks from results, value trap warnings |

AI sections appear only when Ollama is available. Commands work perfectly without it.

## Dedicated Reports

```bash
stockwise report intraday     # Morning trading brief
stockwise report longterm     # Investment memo for SIP
stockwise report portfolio    # Portfolio health review
stockwise report RELIANCE     # Single-stock deep analysis
```

## Model Configuration

Default: `gemma3:12b`

Override with environment variable:

```bash
STOCKWISE_MODEL=qwen3:14b stockwise analyze RELIANCE
STOCKWISE_MODEL=llama3.1:8b stockwise report portfolio
```

## Recommended Models

| Model | Size | Best For |
|-------|------|----------|
| `gemma3:12b` | 8GB | Best general finance reasoning (default) |
| `qwen3:14b` | 9GB | Strong structured analysis |
| `phi4:14b` | 9GB | Excellent at financial metrics |
| `llama3.1:8b` | 5GB | Lighter, faster, decent quality |
| `llama3.3:70b` | 40GB | Best quality (needs 64GB+ RAM) |

## Privacy

- All AI runs locally via Ollama
- No stock data, portfolio, or credentials sent to external services
- Models run on your hardware only
