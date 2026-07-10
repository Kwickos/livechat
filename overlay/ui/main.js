// UI de l'overlay : file d'attente des médias reçus, affichage un par un
// avec fondu, indicateur de connexion.

// Hors Tauri (aperçu dans un navigateur), on bascule en mode démo.
const tauriApi = window.__TAURI__ || null;
const listen = tauriApi ? tauriApi.event.listen : async () => {};
const invoke = tauriApi
  ? tauriApi.core.invoke
  : async () => {
      throw new Error("hors Tauri");
    };

const stage = document.getElementById("stage");
const box = document.getElementById("media-box");
const img = document.getElementById("img");
const video = document.getElementById("video");
const audio = document.getElementById("audio");
const audioCard = document.getElementById("audio-card");
const audioName = document.getElementById("audio-name");
const senderEl = document.getElementById("sender");
const captionEl = document.getElementById("caption");
const statusEl = document.getElementById("status");

let config = {
  display_seconds: 8,
  max_video_seconds: 60,
  volume: 0.5,
  position: "center",
  max_size_percent: 45,
};

const MAX_QUEUE = 20;
const LOAD_TIMEOUT_MS = 15000;

const queue = [];
let busy = false;
let finishing = false;
let fallbackMuted = false; // média forcé en muet par le blocage autoplay
let hideTimer = null;
let guardTimer = null;
let statusTimer = null;

function applyLayout() {
  const pct = Math.min(Math.max(config.max_size_percent, 10), 95);
  document.documentElement.style.setProperty("--max-w", pct + "vw");
  document.documentElement.style.setProperty("--max-h", pct + "vh");

  const positions = ["center", "top-left", "top-right", "bottom-left", "bottom-right"];
  const pos = positions.includes(config.position) ? config.position : "center";
  stage.className = "pos-" + pos;
}

async function init() {
  // Les écouteurs d'abord : un événement émis pendant l'invoke serait perdu.
  await listen("media", (e) => enqueue(e.payload));
  await listen("status", (e) => onStatus(e.payload));
  // Réglages modifiés dans la fenêtre Paramètres : appliqués à chaud,
  // y compris le volume du média en cours de lecture.
  await listen("display-config", (e) => {
    config = Object.assign(config, e.payload);
    applyLayout();
    const v = Math.min(Math.max(config.volume, 0), 1);
    video.volume = v;
    audio.volume = v;
    if (v <= 0) {
      video.muted = audio.muted = true;
    } else if (!fallbackMuted) {
      // ne jamais ré-activer le son d'un média que le navigateur a forcé
      // en muet (blocage autoplay)
      video.muted = audio.muted = false;
    }
  });

  let startup = null;
  try {
    const ui = await invoke("get_config");
    config = Object.assign(config, ui);
    if (ui.startup_state) {
      startup = { state: ui.startup_state, detail: ui.startup_detail };
    }
  } catch (_) {
    // valeurs par défaut
  }

  applyLayout();

  // L'état de démarrage est tiré (pas poussé) : impossible de le rater.
  if (startup) onStatus(startup);

  if (!tauriApi) demo();
}

// Aperçu navigateur : affiche un média factice en boucle.
function demo() {
  document.body.style.background = "#131316";
  const svg =
    "<svg xmlns='http://www.w3.org/2000/svg' width='520' height='300'>" +
    "<rect width='100%' height='100%' rx='24' fill='%230b0b0e'/>" +
    "<rect x='0.5' y='0.5' width='519' height='299' rx='23.5' fill='none' stroke='rgba(255,255,255,0.14)'/>" +
    "<text x='50%' y='44%' font-family='Segoe UI' font-size='44' font-weight='600' fill='%23f2f2f4' text-anchor='middle'>LiveChat</text>" +
    "<text x='50%' y='66%' font-family='Segoe UI' font-size='24' fill='%239c9da6' text-anchor='middle'>Mode aperçu</text></svg>";
  const item = {
    type: "media",
    kind: "image",
    url: "data:image/svg+xml," + svg,
    sender: "Démo",
    caption: "Un exemple de légende",
  };
  showStatus("🟢 Mode aperçu (hors Tauri)", 4000);
  enqueue(item);
  setInterval(() => enqueue(item), 12000);
}

function enqueue(item) {
  if (!item || item.type !== "media" || !item.url) return;
  if (queue.length >= MAX_QUEUE) queue.shift(); // anti-spam : on jette le plus vieux
  queue.push(item);
  pump();
}

const alphaProbe = document.createElement("canvas");

// Inspecte le canal alpha en dessinant le média réduit sur un petit canvas.
// Renvoie true/false, ou null si l'inspection est impossible (média
// cross-origin chargé sans CORS : le canvas est « tainted »).
function probeAlpha(source, w, h) {
  if (!w || !h) return null;
  const scale = 64 / Math.max(w, h);
  const cw = Math.max(1, Math.round(w * scale));
  const ch = Math.max(1, Math.round(h * scale));
  alphaProbe.width = cw;
  alphaProbe.height = ch;
  const ctx = alphaProbe.getContext("2d", { willReadFrequently: true });
  ctx.clearRect(0, 0, cw, ch);
  try {
    ctx.drawImage(source, 0, 0, cw, ch);
    const data = ctx.getImageData(0, 0, cw, ch).data;
    for (let i = 3; i < data.length; i += 4) {
      if (data[i] < 250) return true;
    }
    return false;
  } catch (_) {
    return null;
  }
}

function setAlphaClass(el, alpha) {
  el.classList.toggle("alpha", alpha === true);
}

// Charge le média avec crossOrigin d'abord (indispensable pour inspecter le
// canal alpha via canvas). Si le host refuse CORS, on recharge sans : la
// détection devient impossible et le style rectangulaire est conservé.
function loadInto(el, url) {
  el.crossOrigin = "anonymous";
  el.onerror = () => {
    el.onerror = finish;
    el.removeAttribute("crossorigin");
    el.src = url;
  };
  el.src = url;
}

function pump() {
  if (busy) return;
  const item = queue.shift();
  if (!item) return;
  busy = true;
  show(item);
}

function show(item) {
  clearTimeout(hideTimer); // aucun minuteur orphelin d'un média précédent
  fallbackMuted = false;
  senderEl.textContent = item.sender || "";
  const caption = (item.caption || "").trim();
  const showCaption = caption && !caption.startsWith("http");
  captionEl.textContent = showCaption ? caption : "";
  captionEl.classList.toggle("hidden", !showCaption);

  // garde-fou : si le média ne charge jamais (URL expirée…), on passe au suivant
  clearTimeout(guardTimer);
  guardTimer = setTimeout(finish, LOAD_TIMEOUT_MS);

  img.classList.add("hidden");
  video.classList.add("hidden");
  audioCard.classList.add("hidden");
  setAlphaClass(img, false);
  setAlphaClass(video, false);

  if (item.kind === "video") {
    video.classList.remove("hidden");
    video.volume = Math.min(Math.max(config.volume, 0), 1);
    video.muted = config.volume <= 0;
    video.onended = finish;
    video.onloadeddata = () => {
      const alpha = probeAlpha(video, video.videoWidth, video.videoHeight);
      setAlphaClass(video, alpha);
      if (alpha === false) {
        // certains WebM alpha ouvrent sur une frame pleine : on re-vérifie
        // une fois la lecture entamée
        video.ontimeupdate = () => {
          if (video.currentTime < 0.2) return;
          video.ontimeupdate = null;
          if (probeAlpha(video, video.videoWidth, video.videoHeight)) {
            setAlphaClass(video, true);
          }
        };
      }
      reveal();
      armHide(config.max_video_seconds);
      video.play().catch(() => {
        // l'autoplay avec son a pu être bloqué : on retente en muet
        fallbackMuted = true;
        video.muted = true;
        video.play().catch(finish);
      });
    };
    loadInto(video, item.url);
  } else if (item.kind === "audio") {
    audioCard.classList.remove("hidden");
    audioName.textContent = item.filename || "🔊 Son";
    audio.volume = Math.min(Math.max(config.volume, 0), 1);
    audio.muted = config.volume <= 0;
    audio.onerror = finish;
    audio.onended = finish;
    audio.onloadeddata = () => {
      reveal();
      armHide(config.max_video_seconds);
      audio.play().catch(() => {
        // en cas de blocage autoplay, on laisse au moins la carte visible
        fallbackMuted = true;
        audio.muted = true;
        audio.play().catch(finish);
      });
    };
    audio.src = item.url;
  } else {
    img.classList.remove("hidden");
    img.onload = () => {
      setAlphaClass(img, probeAlpha(img, img.naturalWidth, img.naturalHeight));
      reveal();
      armHide(config.display_seconds);
    };
    loadInto(img, item.url);
  }
}

function reveal() {
  clearTimeout(guardTimer);
  box.classList.add("visible");
}

function armHide(seconds) {
  clearTimeout(hideTimer);
  hideTimer = setTimeout(finish, Math.max(seconds, 1) * 1000);
}

function finish() {
  if (finishing) return; // onended + timer peuvent arriver en double
  finishing = true;
  clearTimeout(hideTimer);
  clearTimeout(guardTimer);
  // Détache les handlers tout de suite : un chargement qui aboutit pendant
  // le fondu de sortie ne doit pas réafficher la boîte ni relancer la vidéo.
  video.onerror = video.onended = video.onloadeddata = video.ontimeupdate = null;
  audio.onerror = audio.onended = audio.onloadeddata = null;
  img.onerror = img.onload = null;
  box.classList.remove("visible");
  setTimeout(() => {
    video.pause();
    video.removeAttribute("src");
    video.load();
    audio.pause();
    audio.removeAttribute("src");
    audio.load();
    img.removeAttribute("src");
    finishing = false;
    busy = false;
    pump();
  }, 300);
}

let stickyUntil = 0;

function onStatus(s) {
  if (!s || !s.state) return;

  // Messages de configuration : prioritaires et durables.
  const configMessages = {
    "config-created": () =>
      "⚙️ Première utilisation : configure le serveur dans la fenêtre Paramètres qui vient de s'ouvrir.",
    "config-create-failed": (d) =>
      "⚠️ Impossible de créer config.toml (" + d + ").",
    "config-invalid": (d) =>
      "⚙️ config.toml invalide (" + d + ") — corrige-le dans la fenêtre Paramètres.",
  };
  if (configMessages[s.state]) {
    stickyUntil = Date.now() + 45000;
    showStatus(configMessages[s.state](s.detail || "?"), 7200000);
    return;
  }
  // Ne pas écraser un message de configuration avec un statut de connexion.
  if (Date.now() < stickyUntil) return;

  const messages = {
    "connecting": "Connexion au serveur…",
    "connected": "🟢 Connecté",
    "disconnected": "🔴 Déconnecté — reconnexion…",
    "update-installed": "✅ Mise à jour installée — redémarrage…",
    "update-none": "✔️ L'overlay est à jour",
    "update-busy": "⏳ Une vérification est déjà en cours…",
  };
  if (s.state === "update-downloading") {
    showStatus("⬇️ Mise à jour " + (s.detail || "") + " — téléchargement…", 60000);
    return;
  }
  if (s.state === "update-error") {
    showStatus("⚠️ Mise à jour impossible : " + (s.detail || "erreur"), 8000);
    return;
  }
  let text = messages[s.state] || s.state;
  if (s.state === "error") {
    text = "🔴 Connexion impossible : " + (s.detail || "erreur inconnue");
  }
  showStatus(text, s.state === "connected" ? 2500 : 8000);
}

function showStatus(text, ms) {
  statusEl.textContent = text;
  statusEl.classList.add("visible");
  clearTimeout(statusTimer);
  statusTimer = setTimeout(() => statusEl.classList.remove("visible"), ms);
}

init();
