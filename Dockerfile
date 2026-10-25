# syntax=docker/dockerfile:1
#
# StockWise in a container:
#
#   docker build -t stockwise .
#   docker run --rm -it -v stockwise-data:/data stockwise quote RELIANCE TCS
#
# Published for linux/amd64 and linux/arm64 as ghcr.io/iamsaquib8/stockwise on
# every release tag (see .github/workflows/release.yml).

# Dependabot keeps both base images current (.github/dependabot.yml).
FROM rust:1.99-slim-trixie AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# Built-in backtest strategies and the `serve` web dashboard, embedded at
# compile time.
COPY examples/strategies ./examples/strategies
COPY web ./web
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked \
    && cp target/release/stockwise /usr/local/bin/stockwise
# Empty data directory, copied into the runtime image with the right owner
# (distroless has no shell to create it there).
RUN mkdir /data

# glibc, libgcc and CA certificates; runs as uid 65532 ("nonroot"), no shell.
FROM gcr.io/distroless/cc-debian13:nonroot
LABEL org.opencontainers.image.title="StockWise" \
      org.opencontainers.image.description="Stock market toolkit for your terminal: Indian (NSE/BSE) and US stocks" \
      org.opencontainers.image.source="https://github.com/iamsaquib8/stockwise" \
      org.opencontainers.image.licenses="MIT" \
      io.modelcontextprotocol.server.name="io.github.iamsaquib8/stockwise"
COPY --from=build /usr/local/bin/stockwise /usr/local/bin/stockwise
COPY --from=build --chown=65532:65532 /data /data
# Portfolio, watchlist, alerts and history live here; mount a volume to keep them.
ENV STOCKWISE_HOME=/data
VOLUME ["/data"]
WORKDIR /data
USER nonroot
ENTRYPOINT ["/usr/local/bin/stockwise"]
CMD ["--help"]
