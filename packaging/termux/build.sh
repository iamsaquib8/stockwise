TERMUX_PKG_HOMEPAGE=https://github.com/iamsaquib8/stockwise
TERMUX_PKG_DESCRIPTION="Stock market toolkit for the terminal: NSE/BSE and US stocks, funds, F&O, backtests"
TERMUX_PKG_LICENSE="MIT"
TERMUX_PKG_MAINTAINER="@iamsaquib8"
TERMUX_PKG_VERSION="0.5.0"
TERMUX_PKG_SRCURL=https://github.com/iamsaquib8/stockwise/archive/refs/tags/v${TERMUX_PKG_VERSION}.tar.gz
TERMUX_PKG_SHA256=06da52fff2625c2781dc2cc16ce0c3bcf1c16851187d030298f17b5359a94f33
TERMUX_PKG_AUTO_UPDATE=true
TERMUX_PKG_BUILD_IN_SRC=true

# Explicit make and install steps: the source tree also has a pyproject.toml
# (for the PyPI wheels), which the default install step would pick up.
termux_step_make() {
	termux_setup_rust
	cargo build --jobs "$TERMUX_PKG_MAKE_PROCESSES" --target "$CARGO_TARGET_NAME" --release --locked
}

termux_step_make_install() {
	install -Dm700 -t "$TERMUX_PREFIX/bin" "target/${CARGO_TARGET_NAME}/release/stockwise"

	# Man pages and shell completions come from the CLI definition, so run a
	# build for the build machine (without the Android toolchain variables).
	mkdir -p generated
	(
		unset CC CXX CFLAGS CXXFLAGS CPPFLAGS LDFLAGS AR AS CPP LD RANLIB READELF STRIP
		cargo build --locked --target-dir target/host
		target/host/debug/stockwise man --dir generated
		target/host/debug/stockwise completions bash > generated/stockwise.bash
		target/host/debug/stockwise completions zsh > generated/_stockwise
		target/host/debug/stockwise completions fish > generated/stockwise.fish
	)
	install -Dm600 -t "$TERMUX_PREFIX/share/man/man1" generated/*.1
	install -Dm600 generated/stockwise.bash "$TERMUX_PREFIX/share/bash-completion/completions/stockwise"
	install -Dm600 generated/_stockwise "$TERMUX_PREFIX/share/zsh/site-functions/_stockwise"
	install -Dm600 generated/stockwise.fish "$TERMUX_PREFIX/share/fish/vendor_completions.d/stockwise.fish"
}
