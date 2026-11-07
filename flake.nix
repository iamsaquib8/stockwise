{
  description = "StockWise: stock market toolkit for your terminal (Indian NSE/BSE and US stocks)";

  # nix run github:iamsaquib8/stockwise -- quote RELIANCE TCS
  # nix profile install github:iamsaquib8/stockwise
  # nix develop          # shell with cargo, clippy, rustfmt and rust-analyzer

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      crate = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package;
    in
    {
      packages = forAllSystems (pkgs: {
        stockwise = pkgs.rustPlatform.buildRustPackage {
          pname = crate.name;
          inherit (crate) version;

          src = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./src
              # Built-in backtest strategies, embedded with include_str!.
              ./examples/strategies
              # The `serve` web dashboard, embedded with include_str!.
              ./web
            ];
          };
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = [ pkgs.installShellFiles ];

          # Man pages and completions come from the binary itself, so they always
          # match the CLI definition.
          postInstall = pkgs.lib.optionalString (pkgs.stdenv.buildPlatform.canExecute pkgs.stdenv.hostPlatform) ''
            $out/bin/stockwise man --dir man
            installManPage man/*.1
            installShellCompletion --cmd stockwise \
              --bash <($out/bin/stockwise completions bash) \
              --fish <($out/bin/stockwise completions fish) \
              --zsh <($out/bin/stockwise completions zsh)
          '';

          meta = {
            inherit (crate) description homepage;
            changelog = "https://github.com/iamsaquib8/stockwise/blob/main/CHANGELOG.md";
            license = pkgs.lib.licenses.mit;
            mainProgram = "stockwise";
            platforms = systems;
          };
        };
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.stockwise;
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.stockwise ];
          packages = with pkgs; [
            clippy
            rustfmt
            rust-analyzer
          ];
        };
      });

      # `nix flake check` builds the package, which runs `cargo test`.
      checks = forAllSystems (pkgs: {
        stockwise = self.packages.${pkgs.stdenv.hostPlatform.system}.stockwise;
      });
    };
}
