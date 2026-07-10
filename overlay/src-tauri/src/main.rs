//! livechat-overlay : fenêtre transparente toujours au premier plan qui
//! affiche les photos/vidéos/sons relayés par livechat-server.
//! Fenêtre Paramètres accessible depuis la zone de notification,
//! mises à jour automatiques via GitHub Releases.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use futures_util::StreamExt;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
#[cfg(any(target_os = "windows", target_os = "linux"))]
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_updater::UpdaterExt;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Config {
    /// self-host ou hosted.
    #[serde(default = "default_connection_mode")]
    connection_mode: String,
    /// Adresse du serveur self-host, ex. "wss://ton-domaine.example/ws"
    server: String,
    /// Mot de passe partagé (le même que côté serveur).
    secret: String,
    /// Durée d'affichage des images, en secondes.
    display_seconds: f64,
    /// Durée maximale des vidéos et des sons, en secondes.
    max_video_seconds: f64,
    /// Volume, de 0.0 (muet) à 1.0.
    volume: f64,
    /// center, top-left, top-right, bottom-left ou bottom-right.
    position: String,
    /// Taille maximale du média, en % de l'écran.
    max_size_percent: f64,
    /// color, dark ou light.
    #[serde(default = "default_icon_variant")]
    icon_variant: String,
    /// Base HTTPS du backend hosted (fixée à DEFAULT_HOSTED_SERVER).
    #[serde(default = "default_hosted_server")]
    hosted_server: String,
    /// Session OAuth Discord courte, renouvelée à chaque connexion.
    #[serde(default)]
    hosted_token: String,
    /// Guild Discord choisie dans le compte connecté.
    #[serde(default)]
    hosted_guild_id: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            connection_mode: default_connection_mode(),
            server: String::new(),
            secret: String::new(),
            display_seconds: 8.0,
            max_video_seconds: 60.0,
            volume: 0.5,
            position: "center".into(),
            max_size_percent: 45.0,
            icon_variant: default_icon_variant(),
            hosted_server: default_hosted_server(),
            hosted_token: String::new(),
            hosted_guild_id: String::new(),
        }
    }
}

impl Config {
    fn can_connect(&self) -> bool {
        match self.connection_mode.as_str() {
            "hosted" => self.hosted_websocket_url().is_some(),
            _ => {
                let server = self.server.trim();
                !self.secret.trim().is_empty()
                    && self.secret != "change-moi"
                    && (server.starts_with("ws://") || server.starts_with("wss://"))
            }
        }
    }

    fn hosted_websocket_url(&self) -> Option<String> {
        let server = self.hosted_server.trim().trim_end_matches('/');
        if self.hosted_token.trim().is_empty() || self.hosted_guild_id.trim().is_empty() {
            return None;
        }
        let ws_base = if let Some(rest) = server.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = server.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            return None;
        };
        let guild = utf8_percent_encode(self.hosted_guild_id.trim(), NON_ALPHANUMERIC);
        let token = utf8_percent_encode(self.hosted_token.trim(), NON_ALPHANUMERIC);
        Some(format!("{ws_base}/ws?guild_id={guild}&token={token}"))
    }

    fn websocket_url(&self) -> Option<String> {
        if self.connection_mode == "hosted" {
            return self.hosted_websocket_url();
        }
        let token = utf8_percent_encode(&self.secret, NON_ALPHANUMERIC);
        let separator = if self.server.contains('?') { '&' } else { '?' };
        Some(format!("{}{}token={token}", self.server, separator))
    }
}

const POSITIONS: &[&str] = &["center", "top-left", "top-right", "bottom-left", "bottom-right"];
const ICON_VARIANTS: &[&str] = &["color", "dark", "light"];
const CONNECTION_MODES: &[&str] = &["self-host", "hosted"];
const TRAY_ID: &str = "main";

fn default_icon_variant() -> String {
    "color".into()
}

fn default_connection_mode() -> String {
    "self-host".into()
}

/// URL fixe du service hébergé (livechat.zaaap.it) — non configurable.
fn default_hosted_server() -> String {
    "https://livechat.zaaap.it".into()
}

/// Normalise la config : l'adresse hosted est toujours le domaine officiel.
fn normalize_config(mut c: Config) -> Config {
    c.hosted_server = default_hosted_server();
    c
}

fn tray_icon(variant: &str) -> Image<'static> {
    let bytes = match variant {
        "dark" => include_bytes!("../icons/dark.png").as_slice(),
        "light" => include_bytes!("../icons/light.png").as_slice(),
        _ => include_bytes!("../icons/color.png").as_slice(),
    };
    Image::from_bytes(bytes).expect("icône embarquée invalide")
}

/// Sérialise la config au format TOML, avec les commentaires d'aide.
fn render_config(c: &Config) -> String {
    let s = |v: &str| toml::Value::String(v.to_string()).to_string();
    format!(
        "# Configuration de l'overlay LiveChat.\n\
         # Modifiable ici ou via la fenêtre Paramètres (icône de la zone de notification).\n\
         \n\
         # Mode de connexion : self-host ou hosted.\n\
         connection_mode = {connection_mode}\n\
         \n\
         # Adresse du serveur self-host (wss://domaine/ws, ou ws://IP:9000/ws en local).\n\
         server = {server}\n\
         \n\
         # Mot de passe partagé (le même que côté serveur).\n\
         secret = {secret}\n\
         \n\
         # Offre hébergée : service fixe (livechat.zaaap.it), session Discord et serveur choisi.\n\
         hosted_server = {hosted_server}\n\
         hosted_token = {hosted_token}\n\
         hosted_guild_id = {hosted_guild_id}\n\
         \n\
         # Durée d'affichage des images, en secondes.\n\
         display_seconds = {display:?}\n\
         \n\
         # Durée maximale des vidéos et des sons, en secondes.\n\
         max_video_seconds = {video:?}\n\
         \n\
         # Volume, de 0.0 (muet) à 1.0.\n\
         volume = {volume:?}\n\
         \n\
         # Position : center, top-left, top-right, bottom-left ou bottom-right.\n\
         position = {position}\n\
         \n\
         # Taille maximale du média, en % de l'écran.\n\
         max_size_percent = {size:?}\n\
         \n\
         # Icône de zone de notification : color, dark ou light.\n\
         icon_variant = {icon_variant}\n",
        connection_mode = s(&c.connection_mode),
        server = s(&c.server),
        secret = s(&c.secret),
        hosted_server = s(&c.hosted_server),
        hosted_token = s(&c.hosted_token),
        hosted_guild_id = s(&c.hosted_guild_id),
        display = c.display_seconds,
        video = c.max_video_seconds,
        volume = c.volume,
        position = s(&c.position),
        size = c.max_size_percent,
        icon_variant = s(&c.icon_variant),
    )
}

/// Écriture atomique : fichier temporaire + sync + rename, pour ne jamais
/// laisser un config.toml tronqué en cas de coupure de courant.
fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("toml.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(contents.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e
    })
}

/// Résultat du chargement de la config, remonté à l'UI.
enum ConfigStatus {
    Loaded,
    Created(PathBuf),
    CreateFailed(PathBuf, String),
    Invalid(PathBuf, String),
}

fn exe_dir() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.to_path_buf())
}

/// %APPDATA%\livechat-overlay\config.toml — toujours accessible en écriture,
/// même quand l'app est installée dans Program Files.
fn appdata_config() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("livechat-overlay").join("config.toml"))
}

/// Charge config.toml : à côté de l'exe (mode portable), sinon dossier
/// courant, sinon AppData. S'il n'existe nulle part, crée un modèle
/// (AppData en priorité). Renvoie aussi le chemin à utiliser pour sauvegarder.
fn load_config() -> (Config, ConfigStatus, PathBuf) {
    let mut paths = Vec::new();
    if let Some(dir) = exe_dir() {
        paths.push(dir.join("config.toml"));
    }
    paths.push(PathBuf::from("config.toml"));
    if let Some(p) = appdata_config() {
        paths.push(p);
    }

    for path in &paths {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        return match toml::from_str::<Config>(&text) {
            Ok(config) => (normalize_config(config), ConfigStatus::Loaded, path.clone()),
            // On signale le fichier cassé plutôt que de continuer en silence.
            Err(e) => (
                Config::default(),
                ConfigStatus::Invalid(path.clone(), e.message().to_string()),
                path.clone(),
            ),
        };
    }

    let mut candidates = Vec::new();
    if let Some(p) = appdata_config() {
        candidates.push(p);
    }
    if let Some(dir) = exe_dir() {
        candidates.push(dir.join("config.toml"));
    }
    let mut last_error = String::new();
    for path in &candidates {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match write_atomic(path, &render_config(&Config::default())) {
            Ok(()) => return (Config::default(), ConfigStatus::Created(path.clone()), path.clone()),
            Err(e) => last_error = e.to_string(),
        }
    }
    let fallback = candidates
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from("config.toml"));
    (
        Config::default(),
        ConfigStatus::CreateFailed(fallback.clone(), last_error),
        fallback,
    )
}

/// Config partagée entre les fenêtres et mise à jour par « Enregistrer ».
struct SharedConfig(Mutex<Config>);

/// Tâche WebSocket en cours. Elle est remplacée lorsqu'une connexion est modifiée.
struct ConnectionTask(Mutex<Option<tauri::async_runtime::JoinHandle<()>>>);

/// Contexte de démarrage (statut de la config et chemin de sauvegarde).
struct Startup {
    state: Option<&'static str>,
    detail: Option<String>,
    config_path: PathBuf,
}

/// Ce que la webview de l'overlay a le droit de connaître : les réglages
/// d'affichage et l'état de démarrage — ni le secret, ni l'adresse du serveur.
#[derive(Clone, Serialize)]
struct UiState {
    display_seconds: f64,
    max_video_seconds: f64,
    volume: f64,
    position: String,
    max_size_percent: f64,
    startup_state: Option<String>,
    startup_detail: Option<String>,
}

fn ui_state(c: &Config, state: Option<&str>, detail: Option<String>) -> UiState {
    UiState {
        display_seconds: c.display_seconds,
        max_video_seconds: c.max_video_seconds,
        volume: c.volume,
        position: c.position.clone(),
        max_size_percent: c.max_size_percent,
        startup_state: state.map(String::from),
        startup_detail: detail,
    }
}

/// Seule la fenêtre Paramètres a le droit d'appeler les commandes sensibles
/// (elles exposent le secret / écrivent sur le disque / redémarrent l'app).
fn ensure_settings(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "settings" {
        Ok(())
    } else {
        Err("accès refusé".into())
    }
}

#[tauri::command]
fn get_config(
    config: tauri::State<'_, SharedConfig>,
    startup: tauri::State<'_, Startup>,
) -> UiState {
    let c = config.0.lock().unwrap();
    ui_state(&c, startup.state, startup.detail.clone())
}

#[tauri::command]
fn get_full_config(
    window: tauri::WebviewWindow,
    config: tauri::State<'_, SharedConfig>,
) -> Result<Config, String> {
    ensure_settings(&window)?;
    Ok(config.0.lock().unwrap().clone())
}

/// Enregistre la config. Renvoie vrai si un redémarrage est nécessaire
/// (adresse du serveur ou secret modifiés).
#[tauri::command]
fn save_config(
    window: tauri::WebviewWindow,
    app: AppHandle,
    config: tauri::State<'_, SharedConfig>,
    startup: tauri::State<'_, Startup>,
    connection_task: tauri::State<'_, ConnectionTask>,
    new_config: Config,
) -> Result<bool, String> {
    ensure_settings(&window)?;

    let mut c = new_config;
    if !CONNECTION_MODES.contains(&c.connection_mode.as_str()) {
        c.connection_mode = default_connection_mode();
    }
    c.server = c.server.trim().to_string();
    c.secret = c.secret.trim().to_string();
    // Toujours le domaine officiel — le champ n'est plus exposé dans l'UI.
    c.hosted_server = default_hosted_server();
    c.hosted_token = c.hosted_token.trim().to_string();
    c.hosted_guild_id = c.hosted_guild_id.trim().to_string();
    if c.connection_mode == "self-host" {
        if !(c.server.starts_with("ws://") || c.server.starts_with("wss://")) {
            return Err("L'adresse du serveur doit commencer par ws:// ou wss://".into());
        }
        if c.secret.is_empty() {
            return Err("Le mot de passe partagé est vide.".into());
        }
    } else if c.hosted_token.is_empty() || c.hosted_guild_id.is_empty() {
        return Err("Connecte Discord puis choisis un serveur avant d'enregistrer.".into());
    }
    c.display_seconds = c.display_seconds.clamp(1.0, 600.0);
    c.max_video_seconds = c.max_video_seconds.clamp(1.0, 3600.0);
    c.volume = c.volume.clamp(0.0, 1.0);
    c.max_size_percent = c.max_size_percent.clamp(10.0, 95.0);
    if !POSITIONS.contains(&c.position.as_str()) {
        c.position = "center".into();
    }
    if !ICON_VARIANTS.contains(&c.icon_variant.as_str()) {
        c.icon_variant = default_icon_variant();
    }

    let path = &startup.config_path;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    write_atomic(path, &render_config(&c))
        .map_err(|e| format!("Écriture impossible ({}) : {e}", path.display()))?;

    let reconnect = config.0.lock().unwrap().websocket_url() != c.websocket_url();

    // Applique immédiatement les réglages d'affichage à l'overlay.
    let _ = app.emit_to("main", "display-config", ui_state(&c, None, None));
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_icon(Some(tray_icon(&c.icon_variant)));
    }
    *config.0.lock().unwrap() = c;
    if reconnect {
        replace_connection_task(&app, &connection_task, config.0.lock().unwrap().clone());
    }

    Ok(false)
}

#[tauri::command]
fn restart_app(window: tauri::WebviewWindow, app: AppHandle) -> Result<(), String> {
    ensure_settings(&window)?;
    app.restart();
}

#[tauri::command]
fn check_updates_now(window: tauri::WebviewWindow, app: AppHandle) -> Result<(), String> {
    ensure_settings(&window)?;
    tauri::async_runtime::spawn(update_task(app, true));
    Ok(())
}

#[tauri::command]
fn get_app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

#[tauri::command]
fn get_autostart(window: tauri::WebviewWindow, app: AppHandle) -> Result<bool, String> {
    ensure_settings(&window)?;
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_autostart(
    window: tauri::WebviewWindow,
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    ensure_settings(&window)?;
    use tauri_plugin_autostart::ManagerExt;
    let launcher = app.autolaunch();
    if enabled {
        launcher.enable().map_err(|e| e.to_string())
    } else {
        launcher.disable().map_err(|e| e.to_string())
    }
}

#[tauri::command]
fn show_test_media(app: AppHandle) {
    let _ = app.emit_to("main", "media", test_media());
}

/// Média factice pour le bouton « Tester l'affichage ».
fn test_media() -> serde_json::Value {
    let svg = r##"<svg xmlns='http://www.w3.org/2000/svg' width='520' height='300'><rect width='100%' height='100%' rx='24' fill='#5865F2'/><text x='50%' y='42%' font-family='Segoe UI, sans-serif' font-size='44' font-weight='bold' fill='white' text-anchor='middle'>LiveChat</text><text x='50%' y='66%' font-family='Segoe UI, sans-serif' font-size='26' fill='white' text-anchor='middle'>L'overlay fonctionne !</text></svg>"##;
    json!({
        "type": "media",
        "kind": "image",
        "url": format!("data:image/svg+xml,{}", utf8_percent_encode(svg, NON_ALPHANUMERIC)),
        "filename": "test.svg",
        "sender": "Test local",
        "caption": ""
    })
}

/// Garde anti-chevauchement : une seule vérification/installation à la fois.
static UPDATE_RUNNING: AtomicBool = AtomicBool::new(false);
/// Version dont le téléchargement a échoué — on ne la retente pas en
/// automatique (sinon toast + re-téléchargement toutes les 4 h).
static FAILED_UPDATE: Mutex<Option<String>> = Mutex::new(None);

/// Vérifie les mises à jour ; si une version plus récente est publiée sur
/// GitHub Releases, la télécharge, l'installe et redémarre l'application.
async fn update_task(app: AppHandle, manual: bool) {
    if UPDATE_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        if manual {
            let _ = app.emit("status", json!({ "state": "update-busy" }));
        }
        return;
    }
    // Remet le drapeau à false sur tous les chemins de sortie (le chemin
    // succès n'en a pas besoin : le process se termine pour l'installation).
    struct Running;
    impl Drop for Running {
        fn drop(&mut self) {
            UPDATE_RUNNING.store(false, Ordering::SeqCst);
        }
    }
    let _running = Running;

    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => {
            if manual {
                let _ = app.emit("status", json!({ "state": "update-error", "detail": e.to_string() }));
            }
            return;
        }
    };
    match updater.check().await {
        Ok(Some(update)) => {
            let version = update.version.clone();
            // En auto, ne retente pas une version qui a déjà échoué.
            if !manual && FAILED_UPDATE.lock().unwrap().as_deref() == Some(version.as_str()) {
                return;
            }
            let _ = app.emit("status", json!({ "state": "update-downloading", "detail": version }));
            match update.download_and_install(|_received, _total| {}, || {}).await {
                Ok(()) => {
                    let _ = app.emit("status", json!({ "state": "update-installed" }));
                    // Laisse le temps d'afficher le message, puis relance
                    // (sur Windows l'installeur NSIS prend le relais).
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    app.restart();
                }
                Err(e) => {
                    *FAILED_UPDATE.lock().unwrap() = Some(version);
                    let _ = app.emit("status", json!({ "state": "update-error", "detail": e.to_string() }));
                }
            }
        }
        Ok(None) => {
            if manual {
                let _ = app.emit("status", json!({ "state": "update-none" }));
            }
        }
        Err(e) => {
            if manual {
                let _ = app.emit("status", json!({ "state": "update-error", "detail": e.to_string() }));
            } else {
                eprintln!("vérification de mise à jour impossible : {e}");
            }
        }
    }
}

/// Boucle de connexion au serveur, avec reconnexion et backoff exponentiel.
async fn ws_task(app: AppHandle, config: Config) {
    let Some(url) = config.websocket_url() else {
        return;
    };

    let mut backoff = Duration::from_secs(2);
    const MAX_BACKOFF: Duration = Duration::from_secs(60);
    // Le serveur envoie un ping toutes les 30 s : 90 s de silence = lien mort
    // (PC sorti de veille, changement de réseau, NAT expiré…).
    const READ_TIMEOUT: Duration = Duration::from_secs(90);

    loop {
        let _ = app.emit("status", json!({ "state": "connecting" }));
        match connect_async(&url).await {
            Ok((mut ws, _resp)) => {
                backoff = Duration::from_secs(2);
                let _ = app.emit("status", json!({ "state": "connected" }));
                loop {
                    match tokio::time::timeout(READ_TIMEOUT, ws.next()).await {
                        Err(_) => break, // silence prolongé : on repart de zéro
                        Ok(None) => break,
                        Ok(Some(msg)) => match msg {
                            Ok(Message::Text(text)) => {
                                let Ok(value) =
                                    serde_json::from_str::<serde_json::Value>(text.as_str())
                                else {
                                    continue;
                                };
                                if value.get("type").and_then(|t| t.as_str()) == Some("media") {
                                    let _ = app.emit_to("main", "media", &value);
                                }
                            }
                            Ok(Message::Close(_)) | Err(_) => break,
                            Ok(_) => {} // Ping/Pong gérés par la lib
                        },
                    }
                }
                let _ = app.emit("status", json!({ "state": "disconnected" }));
            }
            Err(e) => {
                let _ = app.emit(
                    "status",
                    json!({ "state": "error", "detail": e.to_string() }),
                );
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Coupe proprement l'ancienne connexion puis relie l'overlay avec la nouvelle
/// config. Changer de serveur Discord ne demande donc pas de relancer l'app.
fn replace_connection_task(app: &AppHandle, connection_task: &ConnectionTask, config: Config) {
    if let Some(task) = connection_task.0.lock().unwrap().take() {
        task.abort();
    }
    if config.can_connect() {
        let handle = app.clone();
        let task = tauri::async_runtime::spawn(async move {
            ws_task(handle, config).await;
        });
        *connection_task.0.lock().unwrap() = Some(task);
    } else {
        let _ = app.emit("status", json!({ "state": "disconnected" }));
    }
}

fn open_settings(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn main() {
    // rustls 0.23 exige qu'un fournisseur crypto soit installé avant toute
    // connexion TLS ; sans ça, la connexion wss:// panique dans la tâche de
    // fond et l'overlay ne se connecte jamais (sans message d'erreur clair).
    let _ = rustls::crypto::ring::default_provider().install_default();

    let (config, status, config_path) = load_config();

    let (state, detail) = match &status {
        ConfigStatus::Loaded => (None, None),
        ConfigStatus::Created(path) => {
            (Some("config-created"), Some(path.display().to_string()))
        }
        ConfigStatus::CreateFailed(path, e) => (
            Some("config-create-failed"),
            Some(format!("{} — {e}", path.display())),
        ),
        ConfigStatus::Invalid(path, e) => (
            Some("config-invalid"),
            Some(format!("{} — {e}", path.display())),
        ),
    };
    // Config absente, cassée ou incomplète : on ouvre les paramètres au lieu
    // de boucler sur une connexion impossible.
    let connect = matches!(status, ConfigStatus::Loaded) && config.can_connect();
    let startup = Startup {
        state,
        detail,
        config_path,
    };
    let config_for_task = config.clone();

    let mut builder = tauri::Builder::default();
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|_app, _argv, _cwd| {}));
    }

    builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(SharedConfig(Mutex::new(config)))
        .manage(ConnectionTask(Mutex::new(None)))
        .manage(startup)
        .invoke_handler(tauri::generate_handler![
            get_config,
            get_full_config,
            save_config,
            restart_app,
            check_updates_now,
            get_app_version,
            get_autostart,
            set_autostart,
            show_test_media
        ])
        .setup(move |app| {
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            app.deep_link().register_all()?;
            let win = app
                .get_webview_window("main")
                .expect("fenêtre principale absente");

            // Couvre tout l'écran principal.
            if let Ok(Some(monitor)) = win.primary_monitor() {
                let _ = win.set_position(*monitor.position());
                let _ = win.set_size(*monitor.size());
            }
            // Les clics passent à travers l'overlay.
            let _ = win.set_ignore_cursor_events(true);

            // Fermer la fenêtre Paramètres = la cacher (l'app vit dans le tray).
            if let Some(settings) = app.get_webview_window("settings") {
                let sw = settings.clone();
                settings.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = sw.hide();
                    }
                });
            }

            // Icône de zone de notification (tray).
            let settings_item =
                MenuItem::with_id(app, "settings", "Paramètres", true, None::<&str>)?;
            let test = MenuItem::with_id(app, "test", "Tester l'affichage", true, None::<&str>)?;
            let update =
                MenuItem::with_id(app, "update", "Vérifier les mises à jour", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quitter", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&settings_item, &test, &update, &quit])?;
            let tray = TrayIconBuilder::with_id(TRAY_ID)
                .menu(&menu)
                .tooltip("LiveChat Overlay")
                .icon(tray_icon(&config_for_task.icon_variant))
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => app.exit(0),
                    "settings" => open_settings(app),
                    "test" => {
                        let _ = app.emit_to("main", "media", test_media());
                    }
                    "update" => {
                        tauri::async_runtime::spawn(update_task(app.clone(), true));
                    }
                    _ => {}
                });
            tray.build(app)?;

            // Certains jeux réordonnent les fenêtres en passant en plein
            // écran : on réaffirme le premier plan périodiquement.
            {
                let win = win.clone();
                tauri::async_runtime::spawn(async move {
                    loop {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        let _ = win.set_always_on_top(true);
                    }
                });
            }

            // Vérification des mises à jour : au démarrage puis toutes les 4 h.
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(15)).await;
                    loop {
                        update_task(handle.clone(), false).await;
                        tokio::time::sleep(Duration::from_secs(4 * 3600)).await;
                    }
                });
            }

            let handle = app.handle().clone();
            if connect {
                let connection_task = app.state::<ConnectionTask>();
                replace_connection_task(&handle, &connection_task, config_for_task.clone());
            } else {
                // Premier lancement ou config cassée : on guide l'utilisateur.
                open_settings(&handle);
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("erreur au lancement de l'overlay");
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn default_config_does_not_connect() {
        assert!(!Config::default().can_connect());
    }

    #[test]
    fn self_host_config_can_connect() {
        let config = Config {
            server: "wss://example.com/ws".into(),
            secret: "real-secret".into(),
            ..Config::default()
        };
        assert!(config.can_connect());
    }
}
