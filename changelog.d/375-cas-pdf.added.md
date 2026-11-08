- `stockwise import cas FILE.pdf [--password P]` converts a password-protected
  CAS PDF through `casparser` when it is on `PATH` (password from
  `--password`, `STOCKWISE_CAS_PASSWORD` or a hidden prompt), `import cas -`
  reads casparser output from standard input, and a missing `casparser` prints
  a one-line install-and-run command instead of a UTF-8 error
  ([#375](https://github.com/iamsaquib8/stockwise/issues/375)).
