# StockWise hero demo.
# Build first so the binary exists, then record:
#     cargo build --release && vhs assets/demo.tape
# The hidden step below puts the freshly-built binary on PATH, so the demo
# works without a global `cargo install`. Read-only commands only — never
# records live `trade`/`daemon` flows.

Output assets/demo.gif

Set Shell "bash"
Set FontSize 13
Set Width 1180
Set Height 820
Set Padding 18
Set Theme "Dracula"
Set TypingSpeed 45ms

# Hidden setup: make `stockwise` resolve to the locally built binary.
Hide
Type "export PATH=$PWD/target/release:$PWD/target/debug:$PATH"
Enter
Type "clear"
Enter
Show

Type "# Live quotes — US + Indian markets in one CLI"
Enter
Type "stockwise quote AAPL MSFT NVDA -m us"
Enter
Sleep 4s

Type "clear"
Enter
Type "# Major indices at a glance"
Enter
Type "stockwise markets"
Enter
Sleep 4s

Type "clear"
Enter
Type "# Price history with a braille chart + volume bars"
Enter
Type "stockwise history AAPL -m us"
Enter
Sleep 5s
