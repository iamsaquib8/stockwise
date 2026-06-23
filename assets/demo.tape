# StockWise hero demo — regenerate with: vhs assets/demo.tape
# Requires `stockwise` on PATH (cargo install --path .) and network access.
# Read-only commands only — never records live `trade`/`daemon` flows.

Output assets/demo.gif

Set Shell "bash"
Set FontSize 13
Set Width 1180
Set Height 820
Set Padding 18
Set Theme "Dracula"
Set TypingSpeed 45ms

Hide
Type "clear" Enter
Show

Type "# Live quotes — US + Indian markets in one CLI"
Enter
Type "stockwise quote AAPL MSFT NVDA -m us"
Enter
Sleep 4s

Type "clear" Enter
Type "# Major indices at a glance"
Enter
Type "stockwise markets"
Enter
Sleep 4s

Type "clear" Enter
Type "# Price history with a braille chart + volume bars"
Enter
Type "stockwise history AAPL -m us"
Enter
Sleep 5s
