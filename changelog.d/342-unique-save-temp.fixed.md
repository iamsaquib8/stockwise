- Every save now uses its own temp file, so two writes in one process (two
  commands, or one racing the daemon) can no longer rename or delete each
  other's half-written file ([#342](https://github.com/iamsaquib8/stockwise/issues/342)).
  Thanks [@abu-tahir-0](https://github.com/abu-tahir-0).
