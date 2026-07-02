# Image du serveur LiveChat (bot Discord + relais WebSocket),
# prête pour AtlasFlow / Railway / tout hébergeur de conteneurs.

# bookworm explicite : le binaire doit être lié à la même version de GLIBC
# que l'image d'exécution ci-dessous (sinon « GLIBC_2.xx not found »).
FROM rust:1-slim-bookworm AS build
WORKDIR /app
COPY . .
RUN cargo build --release --locked -p livechat-server

FROM debian:bookworm-slim
# ffmpeg : requis par yt-dlp pour fusionner audio/vidéo.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates ffmpeg \
    && rm -rf /var/lib/apt/lists/*
# yt-dlp : binaire autonome (pas besoin de Python). Reconstruire l'image
# met yt-dlp à jour — utile car YouTube change souvent.
ADD https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux /usr/local/bin/yt-dlp
RUN chmod a+rx /usr/local/bin/yt-dlp
COPY --from=build /app/target/release/livechat-server /usr/local/bin/livechat-server

# Pas besoin de root : le serveur ne fait que du réseau + écrire dans /tmp.
USER 65534:65534

# AtlasFlow attend une app qui écoute sur le port 3000.
ENV PORT=3000
EXPOSE 3000
CMD ["livechat-server"]
