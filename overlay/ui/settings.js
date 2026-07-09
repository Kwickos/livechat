// Fenêtre Paramètres : lit/écrit la config via les commandes Rust.

// Hors Tauri (aperçu navigateur) : mode démo, champs remplis avec les défauts.
const tauriApi = window.__TAURI__ || null;
const invoke = tauriApi
  ? tauriApi.core.invoke
  : async () => {
      throw new Error("hors Tauri");
    };
const listen = tauriApi ? tauriApi.event.listen : async () => {};

const $ = (id) => document.getElementById(id);
const serverEl = $("server");
const secretEl = $("secret");
const positionEl = $("position");
const maxSizeEl = $("max-size");
const displaySecondsEl = $("display-seconds");
const maxVideoSecondsEl = $("max-video-seconds");
const volumeEl = $("volume");
const saveStatusEl = $("save-status");
const updateStatusEl = $("update-status");
const iconVariantEls = Array.from(document.querySelectorAll('input[name="icon-variant"]'));

let statusTimer = null;

function refreshLabels() {
  $("size-val").textContent = maxSizeEl.value + " %";
  $("imgsec-val").textContent = displaySecondsEl.value + " s";
  $("vidsec-val").textContent = maxVideoSecondsEl.value + " s";
  $("volume-val").textContent = volumeEl.value === "0" ? "muet" : volumeEl.value + " %";
}

function showSaveStatus(text, isError) {
  saveStatusEl.textContent = text;
  saveStatusEl.classList.toggle("error", !!isError);
  clearTimeout(statusTimer);
  statusTimer = setTimeout(() => (saveStatusEl.textContent = ""), 5000);
}

// Élargit les bornes du curseur si la valeur du fichier les dépasse
// (config.toml édité à la main) : sinon le navigateur la tronquerait
// silencieusement et « Enregistrer » écraserait la valeur voulue.
function setRange(el, value) {
  if (value > Number(el.max)) el.max = value;
  if (value < Number(el.min)) el.min = value;
  el.value = value;
}

async function load() {
  try {
    const cfg = await invoke("get_full_config");
    serverEl.value = cfg.server;
    secretEl.value = cfg.secret;
    positionEl.value = cfg.position;
    maxSizeEl.value = Math.round(cfg.max_size_percent);
    setRange(displaySecondsEl, Math.round(cfg.display_seconds));
    setRange(maxVideoSecondsEl, Math.round(cfg.max_video_seconds));
    volumeEl.value = Math.round(cfg.volume * 100);
    setIconVariant(cfg.icon_variant || "color");
  } catch (_) {
    // valeurs par défaut du HTML (aperçu hors Tauri)
    maxSizeEl.value = 45;
    displaySecondsEl.value = 8;
    maxVideoSecondsEl.value = 60;
    volumeEl.value = 50;
    setIconVariant("color");
  }
  try {
    $("version").textContent = "v" + (await invoke("get_app_version"));
  } catch (_) {
    $("version").textContent = "v0.0.0 (aperçu)";
  }
  try {
    $("autostart").checked = await invoke("get_autostart");
  } catch (_) {
    // indisponible hors Tauri
  }
  refreshLabels();

  // Retour des vérifications de mise à jour (événements globaux).
  await listen("status", (e) => {
    const s = e.payload || {};
    const map = {
      "update-downloading": "⬇️ Téléchargement de la mise à jour " + (s.detail || "") + "…",
      "update-installed": "✅ Installée — redémarrage…",
      "update-none": "✔️ L'application est à jour.",
      "update-busy": "⏳ Une vérification est déjà en cours…",
      "update-error": "⚠️ " + (s.detail || "erreur"),
    };
    if (map[s.state]) updateStatusEl.textContent = map[s.state];
  });
}

async function save() {
  const cfg = {
    server: serverEl.value.trim(),
    secret: secretEl.value.trim(),
    position: positionEl.value,
    max_size_percent: Number(maxSizeEl.value),
    display_seconds: Number(displaySecondsEl.value),
    max_video_seconds: Number(maxVideoSecondsEl.value),
    volume: Number(volumeEl.value) / 100,
    icon_variant: getIconVariant(),
  };
  try {
    const needsRestart = await invoke("save_config", { newConfig: cfg });
    showSaveStatus("Enregistré ✓", false);
    $("restart-bar").classList.toggle("hidden", !needsRestart);
  } catch (e) {
    showSaveStatus(String(e), true);
  }
}

$("save").addEventListener("click", save);
$("restart-now").addEventListener("click", () => invoke("restart_app").catch(() => {}));
$("test").addEventListener("click", () => invoke("show_test_media").catch(() => {}));
$("check-updates").addEventListener("click", () => {
  updateStatusEl.textContent = "Recherche…";
  invoke("check_updates_now").catch((e) => (updateStatusEl.textContent = String(e)));
});
$("toggle-secret").addEventListener("click", () => {
  secretEl.type = secretEl.type === "password" ? "text" : "password";
});
$("autostart").addEventListener("change", (e) => {
  const checked = e.target.checked;
  invoke("set_autostart", { enabled: checked }).catch(() => {
    e.target.checked = !checked; // échec : on revient à l'état précédent
    showSaveStatus("Impossible de modifier le démarrage automatique", true);
  });
});
[maxSizeEl, displaySecondsEl, maxVideoSecondsEl, volumeEl].forEach((el) =>
  el.addEventListener("input", refreshLabels)
);

function getIconVariant() {
  return iconVariantEls.find((el) => el.checked)?.value || "color";
}

function setIconVariant(value) {
  const wanted = ["color", "dark", "light"].includes(value) ? value : "color";
  iconVariantEls.forEach((el) => {
    el.checked = el.value === wanted;
  });
}

load();
