- Reading saved data no longer creates the data directory, so read-only
  commands and the MCP `monitor_status` tool work with a read-only home
  (sandboxed builds such as the nix check).
