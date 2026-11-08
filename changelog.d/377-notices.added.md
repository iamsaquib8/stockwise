- The background worker's post-close sync now sends notices through your
  notification channels: when a held lot is about to turn long-term (7-day
  heads-up) or has turned, and when a held stock reports a new quarter. Each
  is sent once; `stockwise tasks add eod` queues the same sync for the worker
  ([#377](https://github.com/iamsaquib8/stockwise/issues/377)).
