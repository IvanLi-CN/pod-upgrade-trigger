# Stage 1: build the front-end assets
FROM oven/bun:1 AS web-builder
ARG APP_EFFECTIVE_VERSION
WORKDIR /app/web

COPY web/package.json web/bun.lock ./
RUN bun install --frozen-lockfile

COPY web/ ./
ENV VITE_APP_VERSION=${APP_EFFECTIVE_VERSION:-dev}
RUN bun run build

# Stage 2: build the Rust binary
FROM rust:1.91 AS rust-builder
ARG APP_EFFECTIVE_VERSION
WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libsqlite3-dev \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo fetch

COPY . .
ENV APP_EFFECTIVE_VERSION=${APP_EFFECTIVE_VERSION}
RUN cargo build --release --locked

# Stage 3: runtime image
FROM debian:bookworm-slim AS runtime
ARG APP_EFFECTIVE_VERSION

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /srv/app

ENV WEBHOOK_STATE_DIR=/srv/app/data \
    WEBHOOK_WEB_DIST=/srv/app/web \
    APP_EFFECTIVE_VERSION=${APP_EFFECTIVE_VERSION}

COPY --from=rust-builder /app/target/release/webhook-auto-update /usr/local/bin/webhook-auto-update
COPY --from=web-builder /app/web/dist /srv/app/web
RUN mkdir -p /srv/app/data

VOLUME ["/srv/app/data"]
EXPOSE 8080

CMD ["webhook-auto-update"]
