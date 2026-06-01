# syntax=docker/dockerfile:1

# --- Stage 1: build the frontend ---
FROM node:22-alpine AS web
WORKDIR /web
COPY web/package*.json ./
RUN npm ci || npm install
COPY web/ ./
RUN npm run build

# --- Stage 2: build the Rust binary (embeds web/dist) ---
FROM rust:1-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY migrations/ migrations/
# The frontend must exist before building so rust-embed can bake it in.
COPY --from=web /web/dist/ web/dist/
RUN cargo build --release --bin nimbus

# --- Stage 3: minimal runtime ---
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/nimbus /usr/local/bin/nimbus
ENV NIMBUS_BIND_ADDR=0.0.0.0:8080 \
    NIMBUS_DATABASE_URL=sqlite:/data/nimbus.db?mode=rwc
VOLUME ["/data"]
EXPOSE 8080
ENTRYPOINT ["nimbus"]
