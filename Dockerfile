# glory-pulse: build en la VPS vía Coolify (sin cross-compile ni registry).
# Contexto = raíz del repo; el pin de producción es tag git, nunca `main`.
FROM rust:1-bookworm AS build
WORKDIR /app
COPY pulse/Cargo.toml pulse/Cargo.lock ./pulse/
COPY schema ./schema
WORKDIR /app/pulse
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/pulse/target/release/pulse /usr/local/bin/pulse
USER 65534:65534
EXPOSE 3000
CMD ["pulse"]
