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
const connectionModeEls = Array.from(document.querySelectorAll('input[name="connection-mode"]'));
const hostedGuildEl = $("hosted-guild");
const hostedStatusEl = $("hosted-status");
const refreshHostedGuildsEl = $("refresh-hosted-guilds");

/** Service hébergé fixe — pas configurable par l'utilisateur. */
const HOSTED_SERVER_URL = "https://livechat.zaaap.it";

let statusTimer = null;
let hostedToken = "";

// Trace la portion « remplie » du curseur (variable CSS --fill).
function refreshFill(el) {
  const pct = ((el.value - el.min) / (el.max - el.min)) * 100;
  el.style.setProperty("--fill", pct + "%");
}

function refreshLabels() {
  $("size-val").textContent = maxSizeEl.value + " %";
  $("imgsec-val").textContent = displaySecondsEl.value + " s";
  $("vidsec-val").textContent = maxVideoSecondsEl.value + " s";
  $("volume-val").textContent = volumeEl.value === "0" ? "muet" : volumeEl.value + " %";
  [maxSizeEl, displaySecondsEl, maxVideoSecondsEl, volumeEl].forEach(refreshFill);
  // Le fantôme de l'aperçu reflète la taille maximale choisie.
  $("ghost").style.width = maxSizeEl.value + "%";
}

// Aperçu de position : le select natif reste la source de vérité (save/load),
// les pastilles du mini-écran ne font que le piloter.
const screenEl = $("screen");
const posDots = Array.from(document.querySelectorAll(".pos-dot"));

function syncPositionUI() {
  const pos = positionEl.value || "center";
  screenEl.dataset.pos = pos;
  posDots.forEach((dot) => dot.setAttribute("aria-checked", String(dot.dataset.pos === pos)));
}

posDots.forEach((dot) =>
  dot.addEventListener("click", () => {
    positionEl.value = dot.dataset.pos;
    syncPositionUI();
  })
);

// Pastille d'état de connexion (alimentée par les événements « status »).
const connPill = $("conn-pill");
const connText = $("conn-text");
const CONN_STATES = {
  connecting: ["Connexion…", "wait"],
  connected: ["Connecté", "ok"],
  disconnected: ["Déconnecté", "err"],
  error: ["Hors ligne", "err"],
};

function setConnPill(state, detail) {
  const entry = CONN_STATES[state];
  if (!entry) return;
  connPill.hidden = false;
  connPill.className = "status-pill " + entry[1];
  connText.textContent = entry[0];
  if (state === "error" && detail) connPill.title = detail;
  else connPill.removeAttribute("title");
}

function showSaveStatus(text, isError) {
  saveStatusEl.textContent = text;
  saveStatusEl.classList.toggle("error", !!isError);
  clearTimeout(statusTimer);
  statusTimer = setTimeout(() => (saveStatusEl.textContent = ""), 5000);
}

function setConnectionMode(mode) {
  const isSelfHost = mode === "self-host";
  $("self-host-settings").classList.toggle("hidden", !isSelfHost);
  $("hosted-settings").classList.toggle("hidden", isSelfHost);
}

function getConnectionMode() {
  return connectionModeEls.find((el) => el.checked)?.value || "self-host";
}

function setHostedStatus(text, isError = false) {
  hostedStatusEl.textContent = text;
  hostedStatusEl.title = text; // le texte long est tronqué dans la ligne de compte
  hostedStatusEl.classList.toggle("error", isError);
}

function hostedBaseUrl() {
  return HOSTED_SERVER_URL;
}

function setHostedGuilds(guilds, selected = "") {
  hostedGuildEl.replaceChildren();
  if (!guilds.length) {
    const option = new Option("Aucun serveur abonné disponible", "");
    hostedGuildEl.add(option);
    hostedGuildEl.disabled = true;
    return;
  }
  guilds.forEach((guild) => {
    hostedGuildEl.add(new Option(guild.name, guild.guild_id, false, guild.guild_id === selected));
  });
  hostedGuildEl.disabled = false;
}

async function loadHostedGuilds(selected = "") {
  const base = hostedBaseUrl();
  if (!base || !hostedToken) {
    refreshHostedGuildsEl.disabled = true;
    return;
  }
  refreshHostedGuildsEl.disabled = true;
  try {
    const response = await fetch(base + "/api/me/guilds", {
      headers: { Authorization: "Bearer " + hostedToken },
    });
    if (!response.ok) throw new Error("session expirée");
    const guilds = await response.json();
    setHostedGuilds(guilds, selected);
    setHostedStatus(guilds.length ? "Discord connecté ✓" : "Aucun serveur abonné disponible.");
  } catch (error) {
    hostedToken = "";
    setHostedGuilds([]);
    setHostedStatus("Impossible de récupérer les serveurs : " + error.message, true);
  } finally {
    refreshHostedGuildsEl.disabled = !hostedToken;
  }
}

async function startHostedLogin() {
  const openUrl = tauriApi?.opener?.openUrl;
  if (!openUrl) {
    setHostedStatus("La connexion Discord nécessite l'application installée.", true);
    return;
  }
  setHostedStatus("Ouverture de Discord dans ton navigateur…");
  const url =
    hostedBaseUrl() +
    "/auth/discord/start?return_to=" +
    encodeURIComponent("livechat-overlay://oauth");
  try {
    await openUrl(url);
  } catch (error) {
    setHostedStatus(String(error), true);
  }
}

async function handleDeepLink(value) {
  try {
    const url = new URL(value);
    if (url.protocol !== "livechat-overlay:" || url.hostname !== "oauth") return;
    const token = url.searchParams.get("token");
    if (!token) throw new Error("jeton de connexion absent");
    hostedToken = token;
    connectionModeEls.forEach((el) => (el.checked = el.value === "hosted"));
    setConnectionMode("hosted");
    await loadHostedGuilds();
  } catch (error) {
    setHostedStatus("Retour Discord invalide : " + error.message, true);
  }
}

async function installDeepLinkHandler() {
  const deepLink = tauriApi?.deepLink;
  if (!deepLink) return;
  try {
    const urls = await deepLink.getCurrent();
    await Promise.all((urls || []).map(handleDeepLink));
    await deepLink.onOpenUrl((urls) => urls.forEach((url) => handleDeepLink(url)));
  } catch (_) {
    // Le plugin est absent dans l'aperçu navigateur.
  }
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
    const mode = cfg.connection_mode === "hosted" ? "hosted" : "self-host";
    connectionModeEls.forEach((el) => (el.checked = el.value === mode));
    setConnectionMode(mode);
    hostedToken = cfg.hosted_token || "";
    if (mode === "hosted" && hostedToken) await loadHostedGuilds(cfg.hosted_guild_id || "");
  } catch (_) {
    // valeurs par défaut du HTML (aperçu hors Tauri)
    maxSizeEl.value = 45;
    displaySecondsEl.value = 8;
    maxVideoSecondsEl.value = 60;
    volumeEl.value = 50;
    setIconVariant("color");
  }
  syncPositionUI();
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

  // Retour des vérifications de mise à jour + état de connexion (événements globaux).
  await listen("status", (e) => {
    const s = e.payload || {};
    const map = {
      "update-downloading": "Téléchargement de la mise à jour " + (s.detail || "") + "…",
      "update-installed": "Installée — redémarrage…",
      "update-none": "L'application est à jour.",
      "update-busy": "Une vérification est déjà en cours…",
      "update-error": "Mise à jour impossible : " + (s.detail || "erreur"),
    };
    if (map[s.state]) updateStatusEl.textContent = map[s.state];
    setConnPill(s.state, s.detail);
  });
}

async function save() {
  const cfg = {
    connection_mode: getConnectionMode(),
    server: serverEl.value.trim(),
    secret: secretEl.value.trim(),
    hosted_server: hostedBaseUrl(),
    hosted_token: hostedToken,
    hosted_guild_id: hostedGuildEl.value,
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
    return true;
  } catch (e) {
    showSaveStatus(String(e), true);
    return false;
  }
}

$("save").addEventListener("click", save);
$("login-discord").addEventListener("click", startHostedLogin);
refreshHostedGuildsEl.addEventListener("click", () => {
  setHostedStatus("Actualisation des serveurs…");
  loadHostedGuilds(hostedGuildEl.value);
});
hostedGuildEl.addEventListener("change", async () => {
  if (!hostedGuildEl.value) return;
  setHostedStatus("Connexion au serveur sélectionné…");
  if (await save()) setHostedStatus("Serveur changé ✓");
});
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
connectionModeEls.forEach((el) =>
  el.addEventListener("change", (e) => {
    setConnectionMode(e.target.value);
    if (e.target.value === "hosted" && hostedToken) loadHostedGuilds(hostedGuildEl.value);
  })
);
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

async function init() {
  await load();
  await installDeepLinkHandler();
}

init();
