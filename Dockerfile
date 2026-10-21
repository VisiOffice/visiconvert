# Build the Rust API, WASM attribution module, and production web assets.
FROM rust:1.98-trixie AS rust-build
WORKDIR /src
ENV CARGO_BUILD_JOBS=2
RUN rustup target add wasm32-unknown-unknown \
    && cargo install wasm-pack --version 0.15.0 --locked
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo test --locked --workspace \
    && cargo build --locked -p convert-server \
    && cargo build --locked --release -p convert-server \
    && wasm-pack build crates/convert-wasm --target web --release --out-dir /src/web-generated

FROM node:22-trixie-slim AS web-build
WORKDIR /src/web
RUN corepack enable && corepack prepare pnpm@10.12.4 --activate
COPY web/package.json web/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY web ./
COPY --from=rust-build /src/web-generated ./src/generated/attribution
RUN pnpm build

# Run the full browser/database suite without installing tools on the host.
# This target is separate from the small production runtime.
FROM rust-build AS verify
RUN apt-get update \
    && apt-get install -y --no-install-recommends postgresql-17 chromium nodejs \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --user-group --create-home verifier
COPY --from=web-build /src/web /src/web
RUN chown -R verifier:verifier /src /usr/local/cargo
ENV PATH="/usr/lib/postgresql/17/bin:${PATH}" \
    CHROMIUM_PATH=/usr/bin/chromium \
    CARGO_NET_OFFLINE=true
USER verifier
ENTRYPOINT ["node", "web/tests/verify-local.mjs"]

FROM debian:trixie-slim AS app
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --user-group --create-home visiconvert
WORKDIR /srv/visiconvert
COPY --from=rust-build /src/target/release/convert-server /usr/local/bin/convert-server
COPY --from=web-build /src/web/dist ./web/dist
USER visiconvert
ENV LISTEN_ADDR=0.0.0.0:3100
EXPOSE 3100
HEALTHCHECK --interval=15s --timeout=3s --start-period=20s --retries=3 \
    CMD curl --fail --silent http://127.0.0.1:3100/health || exit 1
ENTRYPOINT ["convert-server"]
CMD ["serve"]
