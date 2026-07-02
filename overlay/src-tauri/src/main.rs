//! livechat-overlay : fenêtre transparente toujours au premier plan qui
//! affiche les photos/vidéos relayées par livechat-server.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::time::Duration;

use futures_util::StreamExt;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Config {
    /// Adresse du serveur, ex. "wss://ton-projet.atlasflow.dev/ws"
    server: String,
    /// Mot de passe partagé (le même que côté serveur).
    secret: String,
    /// Durée d'affichage des images, en secondes.
    display_seconds: f64,
    /// Durée maximale d'une vidéo, en secondes.
    max_video_seconds: f64,
    /// Volume des vidéos, de 0.0 (muet) à 1.0.
    volume: f64,
    /// center, top-left, top-right, bottom-left ou bottom-right.
    position: String,
    /// Taille maximale du média, en % de l'écran.
    max_size_percent: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: "ws://127.0.0.1:9000/ws".into(),
            secret: "change-moi".into(),
            display_seconds: 8.0,
            max_video_seconds: 60.0,
            volume: 0.5,
            position: "center".into(),
            max_size_percent: 45.0,
        }
    }
}

const CONFIG_TEMPLATE: &str = r#"# Configuration de l'overlay LiveChat.

# Adresse du serveur (celui qui fait tourner le bot Discord).
# Hébergé sur AtlasFlow : "wss://ton-projet.atlasflow.dev/ws"
# Hébergé chez un pote :  "ws://SON_IP:9000/ws"
server = "ws://ADRESSE_DU_SERVEUR:9000/ws"

# Mot de passe partagé : la même valeur que `secret` côté serveur.
secret = "change-moi"

# Durée d'affichage des images, en secondes.
display_seconds = 8.0

# Durée maximale d'une vidéo, en secondes.
max_video_seconds = 60.0

# Volume des vidéos, de 0.0 (muet) à 1.0.
volume = 0.5

# Position des médias à l'écran :
# center, top-left, top-right, bottom-left ou bottom-right.
position = "center"

# Taille maximale du média, en % de l'écran.
max_size_percent = 45.0
"#;

/// Résultat du chargement de la config, remonté tel quel à l'UI.
enum ConfigStatus {
    Loaded,
    Created(PathBuf),
    CreateFailed(PathBuf, String),
    Invalid(PathBuf, String),
}

fn exe_dir() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.to_path_buf())
}

fn config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(dir) = exe_dir() {
        paths.push(dir.join("config.toml"));
    }
    paths.push(PathBuf::from("config.toml"));
    paths
}

/// Charge config.toml (à côté de l'exe, sinon dossier courant).
/// S'il n'existe nulle part, crée un modèle.
fn load_config() -> (Config, ConfigStatus) {
    let paths = config_paths();

    for path in &paths {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        return match toml::from_str::<Config>(&text) {
            Ok(config) => (config, ConfigStatus::Loaded),
            // On signale le fichier cassé plutôt que de continuer en silence
            // avec des valeurs par défaut trompeuses.
            Err(e) => (
                Config::default(),
                ConfigStatus::Invalid(path.clone(), e.message().to_string()),
            ),
        };
    }

    let mut last_error = String::new();
    for path in &paths {
        match std::fs::write(path, CONFIG_TEMPLATE) {
            Ok(()) => return (Config::default(), ConfigStatus::Created(path.clone())),
            Err(e) => last_error = e.to_string(),
        }
    }
    let path = paths.into_iter().next().unwrap_or_default();
    (Config::default(), ConfigStatus::CreateFailed(path, last_error))
}

/// Ce que la webview a le droit de connaître : les réglages d'affichage et
/// l'état de démarrage — ni le secret, ni l'adresse du serveur.
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

fn ui_state(config: &Config, status: &ConfigStatus) -> UiState {
    let (state, detail) = match status {
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
    UiState {
        display_seconds: config.display_seconds,
        max_video_seconds: config.max_video_seconds,
        volume: config.volume,
        position: config.position.clone(),
        max_size_percent: config.max_size_percent,
        startup_state: state.map(String::from),
        startup_detail: detail,
    }
}

#[tauri::command]
fn get_config(state: tauri::State<'_, UiState>) -> UiState {
    state.inner().clone()
}

/// Média factice pour le bouton « Tester l'affichage » du tray.
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

/// Boucle de connexion au serveur, avec reconnexion et backoff exponentiel.
async fn ws_task(app: AppHandle, config: Config, initial_delay: Duration) {
    // Laisse le message de démarrage (config créée…) visible avant que les
    // statuts de connexion ne prennent la place.
    tokio::time::sleep(initial_delay).await;

    let token: String = utf8_percent_encode(&config.secret, NON_ALPHANUMERIC).to_string();
    let separator = if config.server.contains('?') { '&' } else { '?' };
    let url = format!("{}{}token={}", config.server, separator, token);

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
                                    let _ = app.emit("media", &value);
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

fn main() {
    let (config, status) = load_config();
    let state = ui_state(&config, &status);

    // Config cassée : on n'essaie pas de se connecter avec des valeurs par
    // défaut trompeuses ; l'UI affiche l'erreur, l'utilisateur corrige et relance.
    let connect = !matches!(status, ConfigStatus::Invalid(..));
    // Si un message de démarrage doit s'afficher, on retarde les statuts de
    // connexion pour ne pas l'écraser.
    let initial_delay = match status {
        ConfigStatus::Loaded => Duration::ZERO,
        _ => Duration::from_secs(8),
    };

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![get_config])
        .setup(move |app| {
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

            // Icône de zone de notification (tray).
            let test = MenuItem::with_id(app, "test", "Tester l'affichage", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quitter", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&test, &quit])?;
            let mut tray = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("LiveChat Overlay")
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => app.exit(0),
                    "test" => {
                        let _ = app.emit("media", test_media());
                    }
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
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

            if connect {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    ws_task(handle, config, initial_delay).await;
                });
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("erreur au lancement de l'overlay");
}
