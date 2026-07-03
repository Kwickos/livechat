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
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_updater::UpdaterExt;
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
    /// Durée maximale des vidéos et des sons, en secondes.
    max_video_seconds: f64,
    /// Volume, de 0.0 (muet) à 1.0.
    volume: f64,
    /// center, top-left, top-right, bottom-left ou bottom-right.
    position: String,
    /// Taille maximale du média, en % de l'écran.
    max_size_percent: f64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            // Serveur du groupe précâblé : à l'installation, il ne reste que
            // le mot de passe à saisir dans la fenêtre Paramètres.
            server: "wss://livechat-alexandre-marty-1b609611.atlasflow.dev/ws".into(),
            secret: "change-moi".into(),
            display_seconds: 8.0,
            max_video_seconds: 60.0,
            volume: 0.5,
            position: "center".into(),
            max_size_percent: 45.0,
        }
    }
}

const POSITIONS: &[&str] = &["center", "top-left", "top-right", "bottom-left", "bottom-right"];

/// Sérialise la config au format TOML, avec les commentaires d'aide.
fn render_config(c: &Config) -> String {
    let s = |v: &str| toml::Value::String(v.to_string()).to_string();
    format!(
        "# Configuration de l'overlay LiveChat.\n\
         # Modifiable ici ou via la fenêtre Paramètres (icône de la zone de notification).\n\
         \n\
         # Adresse du serveur (wss://… sur AtlasFlow, ws://IP:9000/ws en local).\n\
         server = {server}\n\
         \n\
         # Mot de passe partagé (le même que côté serveur).\n\
         secret = {secret}\n\
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
         max_size_percent = {size:?}\n",
        server = s(&c.server),
        secret = s(&c.secret),
        display = c.display_seconds,
        video = c.max_video_seconds,
        volume = c.volume,
        position = s(&c.position),
        size = c.max_size_percent,
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
            Ok(config) => (config, ConfigStatus::Loaded, path.clone()),
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

/// Contexte de démarrage (statut de la config, chemin de sauvegarde,
/// valeurs de connexion au lancement — pour savoir si un restart est requis).
struct Startup {
    state: Option<&'static str>,
    detail: Option<String>,
    config_path: PathBuf,
    initial_server: String,
    initial_secret: String,
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
    new_config: Config,
) -> Result<bool, String> {
    ensure_settings(&window)?;

    let mut c = new_config;
    c.server = c.server.trim().to_string();
    if !(c.server.starts_with("ws://") || c.server.starts_with("wss://")) {
        return Err("L'adresse du serveur doit commencer par ws:// ou wss://".into());
    }
    c.secret = c.secret.trim().to_string();
    if c.secret.is_empty() {
        return Err("Le mot de passe partagé est vide.".into());
    }
    c.display_seconds = c.display_seconds.clamp(1.0, 600.0);
    c.max_video_seconds = c.max_video_seconds.clamp(1.0, 3600.0);
    c.volume = c.volume.clamp(0.0, 1.0);
    c.max_size_percent = c.max_size_percent.clamp(10.0, 95.0);
    if !POSITIONS.contains(&c.position.as_str()) {
        c.position = "center".into();
    }

    let path = &startup.config_path;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    write_atomic(path, &render_config(&c))
        .map_err(|e| format!("Écriture impossible ({}) : {e}", path.display()))?;

    let needs_restart =
        c.server != startup.initial_server || c.secret != startup.initial_secret;

    // Applique immédiatement les réglages d'affichage à l'overlay.
    let _ = app.emit_to("main", "display-config", ui_state(&c, None, None));
    *config.0.lock().unwrap() = c;

    Ok(needs_restart)
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
    // Config absente ou cassée : on ne tente pas de se connecter avec des
    // valeurs par défaut trompeuses ; la fenêtre Paramètres s'ouvre à la place.
    let connect = matches!(status, ConfigStatus::Loaded);
    let startup = Startup {
        state,
        detail,
        config_path,
        initial_server: config.server.clone(),
        initial_secret: config.secret.clone(),
    };
    let config_for_task = config.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(SharedConfig(Mutex::new(config)))
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
            let mut tray = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("LiveChat Overlay")
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
                tauri::async_runtime::spawn(async move {
                    ws_task(handle, config_for_task).await;
                });
            } else {
                // Premier lancement ou config cassée : on guide l'utilisateur.
                open_settings(&handle);
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("erreur au lancement de l'overlay");
}
