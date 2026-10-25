# nixpkgs

`package.nix` is a ready-to-submit nixpkgs expression for the released
`v0.5.0` tag (`rustPlatform.buildRustPackage` with `cargoHash`, man pages and
shell completions, `versionCheckHook`, `nix-update-script`). The flake in the
repository root stays the way to build from a checkout; this file is only for
nixpkgs. The `nix` job in `.github/workflows/packaging.yml` builds it with the
flake's nixpkgs on every packaging change.

## Submitting (one time)

1. Add yourself to `maintainers/maintainer-list.nix` in a fork of
   [NixOS/nixpkgs](https://github.com/NixOS/nixpkgs) (`github`, `githubId`,
   `name`, `email`), in its own commit:
   `maintainers: add iamsaquib8`.
2. Copy this file to `pkgs/by-name/st/stockwise/package.nix` and put your
   handle in `maintainers = with lib.maintainers; [ iamsaquib8 ];`. If a newer
   tag is out, bump `version` and refresh both hashes (see *Updating*).
3. Build and check it from the nixpkgs checkout:

   ```bash
   nix-build -A stockwise
   ./result/bin/stockwise --version
   nix-shell -p nixpkgs-review --run "nixpkgs-review wip"
   nix fmt pkgs/by-name/st/stockwise/package.nix
   ```

4. Commit as `stockwise: init at 0.5.0` and open a pull request against
   `master`, filling in the template (tested platforms, `nixpkgs-review`
   result). Reviews can take a few weeks.

## Updating

After it is merged, r-ryantm (the nixpkgs update bot) opens version bumps
on its own because of `passthru.updateScript`. To bump by hand:
`nix-shell maintainers/scripts/update.nix --argstr package stockwise`, or
change `version`, set both hashes to `lib.fakeHash`, build, and paste the
hashes from the errors.

The hashes in this copy were computed by building it in a `nixos/nix`
container against the nixpkgs revision pinned in `flake.lock`.
