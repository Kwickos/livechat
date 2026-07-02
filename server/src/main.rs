//! livechat-server : bot Discord + relais WebSocket.
//!
//! Écoute un salon Discord ; chaque photo/vidéo postée (pièce jointe, lien
//! direct, ou lien YouTube téléchargé via yt-dlp) est rediffusée en JSON à
//! tous les overlays connectés en WebSocket.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::{Deserialize, Deserializer, Serialize};
use serenity::async_trait;
use serenity::http::Http;
use serenity::model::channel::{Message, ReactionType};
use serenity::model::gateway::Ready;
use serenity::model::id::{ChannelId, MessageId};
use serenity::prelude::*;
use tokio::sync::{broadcast, Semaphore};
use tower_http::services::ServeDir;
use tracing::{error, info, warn};
use uuid::Uuid;

/// Configuration lue depuis config.toml (à côté de l'exe ou du dossier courant).
#[derive(Deserialize)]
struct Config {
    /// Token du bot (portail développeur Discord).
    discord_token: String,
    /// ID du salon à écouter (accepte un nombre ou une chaîne).
    #[serde(deserialize_with = "u64_or_string")]
    channel_id: u64,
    /// Adresse d'écoute du serveur WebSocket.
    #[serde(default = "default_bind")]
    bind: String,
    /// Mot de passe partagé avec les overlays.
    secret: String,
    /// URL publique du serveur (ex. https://mon-projet.atlasflow.dev).
    /// Requise pour activer les liens YouTube : les vidéos téléchargées sont
    /// servies à partir de cette adresse. Laissée vide = YouTube désactivé.
    #[serde(default)]
    public_url: Option<String>,
}

fn default_bind() -> String {
    "0.0.0.0:9000".to_string()
}

/// Les IDs Discord dépassent ce que certains éditeurs TOML manipulent bien en
/// nombre ; on accepte donc aussi bien `channel_id = 123` que `channel_id = "123"`.
fn u64_or_string<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Num(u64),
        Str(String),
    }
    match Raw::deserialize(d)? {
        Raw::Num(n) => Ok(n),
        Raw::Str(s) => s
            .trim()
            .parse()
            .map_err(|e| serde::de::Error::custom(format!("channel_id invalide : {e}"))),
    }
}

/// Événement envoyé aux overlays.
#[derive(Serialize)]
struct MediaEvent<'a> {
    r#type: &'static str,
    kind: &'static str,
    url: &'a str,
    filename: &'a str,
    sender: &'a str,
    caption: &'a str,
}

const IMAGE_EXT: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "avif"];
const VIDEO_EXT: &[&str] = &["mp4", "webm", "mov", "m4v"];

fn kind_from_extension(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    let (_, ext) = lower.rsplit_once('.')?;
    if ext == "gif" {
        Some("gif")
    } else if IMAGE_EXT.contains(&ext) {
        Some("image")
    } else if VIDEO_EXT.contains(&ext) {
        Some("video")
    } else {
        None
    }
}

fn kind_from_attachment(content_type: Option<&str>, filename: &str) -> Option<&'static str> {
    if let Some(ct) = content_type {
        let ct = ct.split(';').next().unwrap_or(ct).trim();
        if ct == "image/gif" {
            return Some("gif");
        }
        if ct.starts_with("image/") {
            return Some("image");
        }
        if ct.starts_with("video/") {
            return Some("video");
        }
    }
    kind_from_extension(filename)
}

/// Pour un lien collé dans le message : décide du type via l'extension du chemin.
fn kind_from_url(url: &str) -> Option<&'static str> {
    let no_query = url.split(['?', '#']).next().unwrap_or(url);
    let path = no_query.trim_end_matches('/');
    let last = path.rsplit('/').next().unwrap_or(path);
    kind_from_extension(last)
}

/// Vrai si le mot est un lien YouTube reconnu. Strict volontairement : on ne
/// passe à yt-dlp que des URLs YouTube, jamais un lien arbitraire du salon.
fn is_youtube_url(w: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "https://www.youtube.com/watch",
        "https://youtube.com/watch",
        "https://m.youtube.com/watch",
        "https://music.youtube.com/watch",
        "https://www.youtube.com/shorts/",
        "https://youtube.com/shorts/",
        "https://m.youtube.com/shorts/",
        "https://youtu.be/",
    ];
    PREFIXES.iter().any(|p| w.starts_with(p))
}

/// Télécharge les vidéos YouTube et les sert aux overlays.
struct Downloader {
    dir: PathBuf,
    public_url: String,
    sem: Arc<Semaphore>,
    tx: broadcast::Sender<String>,
}

/// Durée de vie d'une vidéo téléchargée avant suppression automatique.
const MEDIA_TTL: Duration = Duration::from_secs(30 * 60);

impl Downloader {
    async fn handle(
        self: Arc<Self>,
        http: Arc<Http>,
        channel_id: ChannelId,
        msg_id: MessageId,
        url: String,
        sender: String,
    ) {
        // Au plus quelques téléchargements simultanés : on protège la machine.
        let _permit = match self.sem.clone().acquire_owned().await {
            Ok(p) => p,
            Err(_) => return,
        };
        let hourglass = ReactionType::Unicode("⏳".to_string());
        let _ = channel_id.create_reaction(&http, msg_id, hourglass).await;

        let id = Uuid::new_v4().simple().to_string();
        let template = self.dir.join(format!("{id}.%(ext)s"));

        let ok = matches!(
            tokio::time::timeout(Duration::from_secs(180), run_ytdlp(&url, &template)).await,
            Ok(Ok(true))
        );

        let produced = if ok { find_output(&self.dir, &id) } else { None };
        match produced {
            Some(file) => {
                let name = file
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let media_url = format!("{}/media/{}", self.public_url.trim_end_matches('/'), name);
                let event = MediaEvent {
                    r#type: "media",
                    kind: "video",
                    url: &media_url,
                    filename: "",
                    sender: &sender,
                    caption: "",
                };
                if let Ok(json) = serde_json::to_string(&event) {
                    let _ = self.tx.send(json);
                }
                info!("YouTube relayé : {url} -> {media_url}");
                let tv = ReactionType::Unicode("📺".to_string());
                let _ = channel_id.create_reaction(&http, msg_id, tv).await;

                // Suppression différée.
                tokio::spawn(async move {
                    tokio::time::sleep(MEDIA_TTL).await;
                    let _ = tokio::fs::remove_file(&file).await;
                });
            }
            None => {
                warn!("échec du téléchargement YouTube : {url}");
                let cross = ReactionType::Unicode("🚫".to_string());
                let _ = channel_id.create_reaction(&http, msg_id, cross).await;
            }
        }
    }
}

/// Lance yt-dlp. Renvoie Ok(true) si le process se termine avec succès.
async fn run_ytdlp(url: &str, out_template: &Path) -> std::io::Result<bool> {
    // Priorité au H.264/AAC (lu de façon fiable par WebView2), <= 720p.
    const FORMAT: &str = "best[ext=mp4][height<=720]/bestvideo[ext=mp4][vcodec^=avc1][height<=720]+bestaudio[ext=m4a]/best[height<=720]";

    let status = tokio::process::Command::new("yt-dlp")
        .arg("--no-playlist")
        .arg("--no-cache-dir")
        .arg("--quiet")
        .arg("--no-warnings")
        .args(["-f", FORMAT])
        .args(["--merge-output-format", "mp4"])
        .args(["--max-filesize", "150M"])
        // Ignore les vidéos de plus de 30 min (sinon fichier/traffic énormes).
        .args(["--match-filter", "duration < 1800"])
        .args(["--socket-timeout", "20"])
        .arg("-o")
        .arg(out_template)
        .arg("--")
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await?;
    Ok(status.success())
}

/// Retrouve le fichier produit par yt-dlp pour un identifiant donné.
fn find_output(dir: &Path, id: &str) -> Option<PathBuf> {
    let exact = dir.join(format!("{id}.mp4"));
    if exact.exists() {
        return Some(exact);
    }
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
            if name.starts_with(id) && !name.ends_with(".part") {
                return Some(path);
            }
        }
    }
    None
}

struct Handler {
    channel_id: u64,
    tx: broadcast::Sender<String>,
    downloader: Option<Arc<Downloader>>,
}

impl Handler {
    fn relay(&self, event: &MediaEvent) {
        match serde_json::to_string(event) {
            // Err = aucun overlay connecté pour l'instant, pas grave.
            Ok(json) => {
                let _ = self.tx.send(json);
            }
            Err(e) => error!("sérialisation impossible : {e}"),
        }
    }
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot || msg.channel_id.get() != self.channel_id {
            return;
        }
        let sender = msg.author.display_name();
        let mut relayed = 0u32;

        for att in &msg.attachments {
            let Some(kind) = kind_from_attachment(att.content_type.as_deref(), &att.filename)
            else {
                continue;
            };
            self.relay(&MediaEvent {
                r#type: "media",
                kind,
                url: &att.url,
                filename: &att.filename,
                sender,
                caption: &msg.content,
            });
            relayed += 1;
        }

        // Liens dans le message : médias directs (https) et YouTube.
        let mut yt_launched = 0u32;
        for word in msg.content.split_whitespace() {
            // Lien YouTube -> téléchargement asynchrone via yt-dlp.
            if is_youtube_url(word) {
                if let Some(dl) = &self.downloader {
                    if yt_launched < 3 {
                        yt_launched += 1;
                        let dl = dl.clone();
                        let http = ctx.http.clone();
                        let channel_id = msg.channel_id;
                        let msg_id = msg.id;
                        let url = word.to_string();
                        let who = sender.to_string();
                        tokio::spawn(async move {
                            dl.handle(http, channel_id, msg_id, url, who).await;
                        });
                    }
                }
                continue;
            }
            // Lien direct https vers une image/vidéo (http ignoré : bloqué par
            // l'overlay).
            if !word.starts_with("https://") {
                continue;
            }
            let Some(kind) = kind_from_url(word) else {
                continue;
            };
            self.relay(&MediaEvent {
                r#type: "media",
                kind,
                url: word,
                filename: "",
                sender,
                caption: "",
            });
            relayed += 1;
        }

        if relayed > 0 {
            info!(
                "{relayed} média(s) de {sender} relayé(s) à {} overlay(s)",
                self.tx.receiver_count()
            );
            let _ = msg.react(&ctx.http, '📺').await;
        }
    }

    async fn ready(&self, _ctx: Context, ready: Ready) {
        info!("Bot Discord connecté en tant que {}", ready.user.name);
    }
}

struct AppState {
    tx: broadcast::Sender<String>,
    secret: String,
}

#[derive(Deserialize)]
struct WsParams {
    #[serde(default)]
    token: String,
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<WsParams>,
    State(state): State<Arc<AppState>>,
) -> Result<Response, StatusCode> {
    if params.token != state.secret {
        warn!("connexion overlay refusée (mauvais token)");
        return Err(StatusCode::UNAUTHORIZED);
    }
    let rx = state.tx.subscribe();
    let count = state.tx.receiver_count();
    Ok(ws.on_upgrade(move |socket| client_loop(socket, rx, count)))
}

async fn client_loop(mut socket: WebSocket, mut rx: broadcast::Receiver<String>, count: usize) {
    info!("overlay connecté ({count} au total)");
    let mut keepalive = tokio::time::interval(Duration::from_secs(30));
    keepalive.tick().await; // le premier tick est immédiat

    loop {
        tokio::select! {
            res = socket.recv() => match res {
                Some(Ok(_)) => {} // rien à recevoir des overlays
                Some(Err(_)) | None => break,
            },
            res = rx.recv() => match res {
                Ok(json) => {
                    if socket.send(WsMessage::Text(json.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("overlay trop lent, {n} message(s) sauté(s)");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            // Ping périodique pour que les box/NAT ne coupent pas la connexion.
            _ = keepalive.tick() => {
                if socket.send(WsMessage::Ping(Bytes::new())).await.is_err() {
                    break;
                }
            }
        }
    }
    info!("overlay déconnecté");
}

/// Config par variables d'environnement, pour un hébergement type PaaS
/// (AtlasFlow, Railway…) : DISCORD_TOKEN, CHANNEL_ID, SECRET,
/// et en option PUBLIC_URL, BIND ou PORT (défaut 0.0.0.0:3000).
fn config_from_env() -> Option<Config> {
    let token = std::env::var("DISCORD_TOKEN").ok();
    let channel = std::env::var("CHANNEL_ID").ok();
    let secret = std::env::var("SECRET").ok();
    if token.is_none() && channel.is_none() && secret.is_none() {
        return None;
    }

    let mut missing = Vec::new();
    if token.is_none() {
        missing.push("DISCORD_TOKEN");
    }
    if channel.is_none() {
        missing.push("CHANNEL_ID");
    }
    if secret.is_none() {
        missing.push("SECRET");
    }
    if !missing.is_empty() {
        eprintln!("Variables d'environnement manquantes : {}", missing.join(", "));
        std::process::exit(1);
    }

    let channel_id = match channel.unwrap().trim().parse() {
        Ok(id) => id,
        Err(e) => {
            eprintln!("CHANNEL_ID invalide : {e}");
            std::process::exit(1);
        }
    };
    let bind = std::env::var("BIND").unwrap_or_else(|_| {
        let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
        format!("0.0.0.0:{port}")
    });

    Some(Config {
        discord_token: token.unwrap(),
        channel_id,
        bind,
        secret: secret.unwrap(),
        public_url: std::env::var("PUBLIC_URL").ok().filter(|s| !s.trim().is_empty()),
    })
}

fn read_config() -> Config {
    let mut paths = vec![std::path::PathBuf::from("config.toml")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            paths.push(dir.join("config.toml"));
        }
    }
    for path in &paths {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        match toml::from_str(&text) {
            Ok(config) => return config,
            Err(e) => {
                eprintln!("Erreur dans {} : {e}", path.display());
                std::process::exit(1);
            }
        }
    }
    if let Some(config) = config_from_env() {
        return config;
    }
    eprintln!(
        "Aucune configuration trouvée.\n\
         Soit : copie server/config.example.toml vers config.toml (à côté de \
         l'exe ou dans le dossier courant) et remplis-le.\n\
         Soit : définis les variables d'environnement DISCORD_TOKEN, \
         CHANNEL_ID et SECRET (hébergement type AtlasFlow/Railway)."
    );
    std::process::exit(1);
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,serenity=warn".into()),
        )
        .init();

    let config = read_config();

    // Un secret vide ou laissé au modèle désactiverait l'authentification
    // sur un serveur accessible depuis internet : on refuse de démarrer.
    if config.secret.trim().is_empty() || config.secret == "change-moi" {
        eprintln!(
            "`secret` est vide ou laissé à la valeur d'exemple « change-moi » : \
             choisis un vrai mot de passe partagé."
        );
        std::process::exit(1);
    }
    if config.discord_token.trim().is_empty() || config.discord_token == "COLLE_TON_TOKEN_ICI" {
        eprintln!("`discord_token` est vide ou laissé à la valeur d'exemple.");
        std::process::exit(1);
    }

    let (tx, _) = broadcast::channel::<String>(64);

    // Dossier des vidéos YouTube téléchargées (nettoyé au fil de l'eau).
    let media_dir = std::env::temp_dir().join("livechat-media");
    if let Err(e) = std::fs::create_dir_all(&media_dir) {
        warn!("dossier média « {} » non créé : {e}", media_dir.display());
    }

    // Le téléchargement YouTube n'est possible que si on connaît l'URL
    // publique du serveur (pour construire des liens que les overlays peuvent
    // atteindre).
    let downloader = match &config.public_url {
        Some(public_url) => {
            info!("YouTube activé (téléchargement via yt-dlp)");
            Some(Arc::new(Downloader {
                dir: media_dir.clone(),
                public_url: public_url.clone(),
                sem: Arc::new(Semaphore::new(2)),
                tx: tx.clone(),
            }))
        }
        None => {
            info!("YouTube désactivé (définis PUBLIC_URL pour l'activer)");
            None
        }
    };

    let state = Arc::new(AppState {
        tx: tx.clone(),
        secret: config.secret.clone(),
    });

    let app = Router::new()
        .route("/ws", get(ws_handler))
        // Les hébergeurs type AtlasFlow vérifient que GET / répond 2xx.
        .route("/", get(|| async { "LiveChat server OK" }))
        .route("/health", get(|| async { "ok" }))
        // Vidéos YouTube téléchargées (supporte les requêtes Range pour la lecture).
        .nest_service("/media", ServeDir::new(&media_dir))
        .with_state(state);

    let listener = match tokio::net::TcpListener::bind(&config.bind).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Impossible d'écouter sur {} : {e}", config.bind);
            std::process::exit(1);
        }
    };
    info!("Serveur WebSocket en écoute sur {}", config.bind);
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            error!("serveur WebSocket arrêté : {e}");
        }
    });

    let intents = GatewayIntents::GUILD_MESSAGES | GatewayIntents::MESSAGE_CONTENT;
    let mut client = match Client::builder(&config.discord_token, intents)
        .event_handler(Handler {
            channel_id: config.channel_id,
            tx,
            downloader,
        })
        .await
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Création du client Discord impossible : {e}");
            std::process::exit(1);
        }
    };

    info!("Connexion à Discord…");
    if let Err(e) = client.start().await {
        error!(
            "Client Discord arrêté : {e:?}\n\
             (si l'erreur parle de « disallowed intents », active « Message Content Intent » \
             dans le portail développeur Discord, onglet Bot)"
        );
    }
}
