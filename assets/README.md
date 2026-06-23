# Demo assets

The animated demo is generated with [VHS](https://github.com/charmbracelet/vhs)
from the `.tape` scripts here, so it's fully reproducible.

## Regenerate the GIF

```bash
# 1. Install VHS (https://github.com/charmbracelet/vhs#installation)
brew install vhs                              # macOS
# 2. Build the binary (the tape adds target/ to PATH for you)
cargo build --release
# 3. Record — writes assets/demo.gif
vhs assets/demo.tape
```

Then reference it in the README with `![StockWise demo](assets/demo.gif)`.

The tape uses **read-only** commands only (`quote`, `markets`, `history`) and a
hidden step that puts the freshly-built binary on `PATH` — so the recorded
commands run for real instead of printing "command not found". It never records
live `trade`/`daemon` flows. Output depends on live market data, so each
recording differs slightly.
