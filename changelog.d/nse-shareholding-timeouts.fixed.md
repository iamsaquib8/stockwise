- `research` and `verdict`: the NSE shareholding read now times out per request (3 seconds each, 8
  seconds in all) and keeps the quarters it has read, saying how many in a warning, instead of dropping
  all of them when a cold start is slow. The verdict no longer loses its shareholding pillar for that.
