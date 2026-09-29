# glory-pulse: build FUERA de la VPS (GitHub Actions → GHCR; Rev.4 299A-12).
# [por qué] Compilar en la VPS productiva reinició dockerd el 2026-09-20; la
# VPS solo hace pull por tag fijo. Contexto = raíz del repo.
FROM rust:1.98-bookworm AS build
WORKDIR /app/pulse
COPY pulse/Cargo.toml pulse/Cargo.lock ./
COPY pulse/src ./src
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/pulse/target/release/pulse /usr/local/bin/pulse
USER 65534:65534
EXPOSE 3000
CMD ["pulse"]
