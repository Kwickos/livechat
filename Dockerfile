# Image du serveur LiveChat (bot Discord + relais WebSocket),
# prête pour AtlasFlow / Railway / tout hébergeur de conteneurs.

FROM rust:1-slim AS build
WORKDIR /app
COPY . .
RUN cargo build --release --locked -p livechat-server

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/livechat-server /usr/local/bin/livechat-server

# Pas besoin de root : le serveur ne fait que du réseau sortant + un port haut.
USER 65534:65534

# AtlasFlow attend une app qui écoute sur le port 3000.
ENV PORT=3000
EXPOSE 3000
CMD ["livechat-server"]
