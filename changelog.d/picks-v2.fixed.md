- `picks` no longer puts stocks with broken or one-off figures at the top. Size
  and liquidity floors apply (India ₹500 crore and ₹2 crore traded a day, US
  $1 billion and $5 million, lower for `--cap small` / `--cap micro`).
  Fundamentals and a year of prices are fetched for the leaders before the
  final ranking. Losses and negative book value rank last, P/E below 3 or ROE
  above 80% are listed apart to check, stocks with under 60% of their data stay
  out, and at most 3 per sector are listed. There is a new risk pillar
  (volatility, drawdown, beta). See `docs/methodology.md`.
