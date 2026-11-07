- Yahoo Finance: when the cookie and crumb handshake fails (for example while Yahoo is rate limiting you),
  StockWise now remembers the failure for 45 seconds instead of repeating up to six requests and their
  waits for every later call, so a command that makes many fundamentals calls fails fast.
