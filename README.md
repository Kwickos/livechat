# LiveChat

Affiche en direct, chez toi et tes potes, les photos/vidéos postées dans un
salon Discord — par-dessus vos jeux.

```
  Un pote poste une image        Le serveur (chez toi)          Chez tout le monde
  dans #le-salon          ──►    bot Discord + WebSocket  ──►   overlay transparent
                                                                qui affiche le média
```

Deux programmes :

| Programme | Qui le lance | Rôle |
|---|---|---|
| `livechat-server` | Toi (une seule instance) | Bot Discord : écoute le salon et rediffuse les médias en WebSocket |
| `livechat-overlay` | Chaque pote (et toi) | Fenêtre transparente au premier plan qui affiche les médias reçus |

Deux modes sont prévus :

| Mode | Pour qui | Fonctionnement |
|---|---|---|
| Self-host | Ceux qui veulent gérer leur infra | Ils lancent leur serveur + leur bot Discord. |
| Hosted | Ceux qui veulent juste installer | Ils ajoutent le bot officiel, paient l'abonnement, puis choisissent leur serveur dans l'overlay. |

Le mode hosted vit dans un backend séparé. Ce dépôt reste self-hostable et contient
l'overlay public.

---

## 1. Créer le bot Discord (une seule fois)

1. Va sur https://discord.com/developers/applications → **New Application** → donne-lui un nom (ex. « LiveChat »).
2. Onglet **Bot** :
   - **Reset Token** → copie le token (c'est le `discord_token` de la config serveur — ne le partage jamais).
   - Dans **Privileged Gateway Intents**, active **Message Content Intent** ← indispensable, sinon le bot ne voit pas les pièces jointes.
3. Onglet **OAuth2** → copie le **Client ID**, puis invite le bot sur ton serveur Discord avec cette URL (remplace `TON_CLIENT_ID`) :

   ```
   https://discord.com/oauth2/authorize?client_id=TON_CLIENT_ID&scope=bot&permissions=66624
   ```

   (permissions : voir le salon, lire l'historique, ajouter des réactions — le bot réagit 📺 quand il relaie un média)
4. Récupère l'**ID du salon** : Discord → Paramètres → Avancés → active le **Mode développeur**, puis clic droit sur le salon → **Copier l'identifiant du salon**.

## 2. Lancer le serveur

### Option A — Docker Compose

```bash
cp .env.example .env
```

Édite `.env` :

| Variable | Valeur |
|---|---|
| `DISCORD_TOKEN` | le token du bot |
| `CHANNEL_ID` | l'ID du salon |
| `SECRET` | le mot de passe partagé avec les overlays |
| `PUBLIC_URL` | *(optionnel)* l'URL publique du serveur — active les liens YouTube |
| `LIVECHAT_PORT` | port exposé sur ta machine, `9000` par défaut |

Puis lance :

```bash
docker compose up -d --build
```

Dans le `config.toml` des overlays :

```toml
server = "ws://TON_IP:9000/ws"
```

Si tu mets un reverse proxy TLS devant le serveur :

```toml
server = "wss://ton-domaine.example/ws"
```

### Option B — hébergeur de conteneurs

Pousse ce dépôt sur GitHub puis déploie le `Dockerfile` chez ton hébergeur
de conteneurs. Ajoute les mêmes variables que dans `.env.example`.

### Option C — depuis le code

```powershell
# dans le dossier du projet
copy server\config.example.toml config.toml
# édite config.toml : discord_token, channel_id, secret
cargo run --release -p livechat-server
```

Au démarrage tu dois voir `Bot Discord connecté en tant que …`.

**Réseau** : pour que les potes se connectent depuis chez eux :
- Redirige le port `9000` (TCP) de ta box vers ton PC ;
- Autorise `livechat-server.exe` dans le pare-feu Windows (une popup s'affiche au premier lancement) ;
- Donne aux potes ton IP publique (https://ifconfig.me) ou un nom DynDNS ;
- Dans le `config.toml` des overlays : `server = "ws://TON_IP:9000/ws"`.

## 3. Installer l'overlay (chez chaque pote)

Chaque pote télécharge **`LiveChat.Overlay_x.y.z_x64-setup.exe`** depuis la
[dernière release GitHub](https://github.com/Kwickos/livechat/releases/latest) et l'installe
(pas besoin de droits admin — installation par utilisateur).

> Windows SmartScreen affichera « Éditeur inconnu » à la première installation
> (l'app n'a pas de certificat payant) : cliquer **Informations complémentaires → Exécuter quand même**.

Au premier lancement, la **fenêtre Paramètres s'ouvre automatiquement** :

1. Renseigner l'**adresse du serveur** (ex. `wss://ton-domaine.example/ws`) et le **mot de passe partagé** ;
2. Régler si besoin position, taille, durées et volume ;
3. **Enregistrer** → **Redémarrer maintenant**.

L'app vit ensuite dans la zone de notification (à côté de l'horloge) :
**Paramètres**, **Tester l'affichage**, **Vérifier les mises à jour**, **Quitter**.
Un badge « 🟢 Connecté » s'affiche en haut à droite quand la connexion est établie.

**Mises à jour automatiques** : l'overlay vérifie GitHub Releases au démarrage puis
toutes les 4 h, télécharge, installe et se relance tout seul. Personne n'a rien à faire.

La config est stockée dans `%APPDATA%\livechat-overlay\config.toml` (ou à côté de
l'exe en mode portable) — éditable à la main, mais la fenêtre Paramètres suffit.

> L'overlay nécessite WebView2, préinstallé sur Windows 11 (et sur Windows 10 récent).

## Publier une nouvelle version de l'overlay

1. Modifier `version` dans `overlay/src-tauri/tauri.conf.json` **et** `overlay/src-tauri/Cargo.toml` (ex. `0.2.1`) ;
2. Commit + push, puis :
   ```powershell
   git tag v0.2.1
   git push origin v0.2.1
   ```
3. GitHub Actions compile l'installeur signé et publie la release (~10 min).
   Tous les overlays installés se mettront à jour automatiquement.

(Les clés de signature : la privée est dans les secrets GitHub du repo et dans
`~\.tauri\livechat-overlay.key` — à ne jamais perdre ni commiter ; la publique est dans `tauri.conf.json`.)

## Liens YouTube (optionnel)

Si tu définis `PUBLIC_URL` (variable d'env, ou `public_url` dans le `config.toml` du serveur), le serveur **télécharge** les liens YouTube postés dans le salon (via `yt-dlp`, en ≤ 720p H.264) puis les rediffuse comme une vidéo normale — l'overlay les joue en autoplay, sans habillage YouTube. Rien à changer côté overlay.

Le bot réagit au message : ⏳ pendant le téléchargement, 📺 quand c'est prêt, 🚫 en cas d'échec (vidéo > 30 min, > 150 Mo, ou indisponible).

- **Docker / hébergeur de conteneurs** : `yt-dlp` et `ffmpeg` sont installés automatiquement par le `Dockerfile`. Il suffit d'ajouter `PUBLIC_URL`.
- **En local** : installe `yt-dlp` et `ffmpeg` (`winget install yt-dlp.yt-dlp Gyan.FFmpeg`), et mets `public_url = "http://TON_IP:9000"` dans le `config.toml`.
- Ça consomme de la bande passante (chaque vidéo est renvoyée à chaque overlay) et du stockage temporaire (fichiers supprimés après 30 min). Pour une bande de potes, ça reste négligeable.
- Télécharger des vidéos YouTube va à l'encontre des CGU de YouTube ; à n'utiliser qu'entre potes, à titre privé.

## Compiler depuis zéro

Prérequis (Windows) :
- [Rust](https://rustup.rs) ;
- Visual Studio Build Tools 2022 avec la charge de travail C++ (`winget install Microsoft.VisualStudio.2022.BuildTools`).

Puis `cargo build --release` à la racine compile les deux programmes.

## Limites connues

- **Plein écran exclusif** : l'overlay s'affiche par-dessus les jeux en *fenêtré sans bordure* (borderless) — le mode par défaut de la plupart des jeux récents — mais **pas** en *plein écran exclusif*. C'est une limite de Windows (même l'overlay Discord officiel ne le fait qu'en s'injectant dans le jeu). Si l'overlay n'apparaît pas : passe le jeu en « Fenêtré sans bordure ».
- **Liens Discord temporaires** : les URLs des pièces jointes Discord expirent après ~24 h. Aucun impact en direct (les overlays affichent le média à la seconde où il est posté), mais un overlay lancé plus tard ne « rattrape » pas les anciens médias.
- **Types supportés** : images (png, jpg, webp, gif, bmp, avif), vidéos (mp4, webm, mov, m4v) et sons (mp3, wav, ogg, m4a, flac, opus — affichés avec une petite carte animée), en pièce jointe ou en lien direct `https://` collé dans le message (les liens `http://` sont ignorés) ; **liens YouTube** si `PUBLIC_URL` est défini (voir plus haut). Les GIF Tenor/Giphy intégrés (choisis via le sélecteur GIF de Discord) ne sont pas encore gérés.
- **Sécurité** : la connexion est en `ws://` (non chiffrée) avec un mot de passe partagé — suffisant entre potes. Pour du chiffrement, mets un reverse proxy TLS (Caddy, nginx) devant le serveur et utilise `wss://`.

## Dépannage

| Problème | Cause probable |
|---|---|
| `disallowed intents` au démarrage du serveur | **Message Content Intent** pas activé dans le portail développeur (étape 1.2) |
| Le bot ne réagit pas 📺 aux messages | Mauvais `channel_id`, ou le bot n'a pas accès au salon |
| L'overlay affiche « Connexion impossible : … 401 Unauthorized » | Le `secret` de l'overlay ne correspond pas à celui du serveur |
| L'overlay affiche « Connexion impossible » (autre erreur) | Mauvaise adresse `server`, port pas redirigé, ou pare-feu |
| Le serveur refuse de démarrer : « secret… change-moi » | Choisis un vrai mot de passe dans `secret`/`SECRET` — la valeur d'exemple est refusée |
| L'overlay affiche « Déconnecté » puis se reconnecte | Normal après une coupure réseau — la reconnexion est automatique |
| Rien ne s'affiche en jeu | Jeu en plein écran exclusif → passe en fenêtré sans bordure |
| La vidéo n'a pas de son | Monte `volume` dans le config.toml de l'overlay (0.0 = muet) |
