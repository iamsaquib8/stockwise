- `mf nav` and `mf history`: when a download fails, a saved copy is used only if it is at most 7 days
  old (older is an error that says so), and the output says it is a saved copy and from which day,
  with a `stale` entry in `--json`. Before, any age was accepted and only a line on stderr said so.
