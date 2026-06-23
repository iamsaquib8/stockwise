# Demo assets

The animated demo is generated with [VHS](https://github.com/charmbracelet/vhs)
from the `.tape` scripts here, so it's fully reproducible.

## Regenerate the GIF

```bash
# 1. Install VHS (https://github.com/charmbracelet/vhs#installation)
brew install vhs            # macOS
# 2. Put stockwise on PATH
cargo install --path .
# 3. Record
vhs assets/demo.tape        # writes assets/demo.gif
```

The tapes use **read-only** commands only (`quote`, `markets`, `technical`,
`deep`). They never record live `trade` or `daemon` flows. Output depends on
live market data, so each recording will differ slightly.
