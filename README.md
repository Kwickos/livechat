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

### Option A — hébergé sur AtlasFlow (recommandé : pas de port à ouvrir)

1. Pousse ce dépôt sur GitHub (repo privé conseillé — le token n'y est pas, mais autant rester discret).
2. Sur [AtlasFlow](https://atlasflow.com) : crée un compte, choisis un plan (Hobby à 5 $/mois suffit largement — le serveur est une petite instance quasi idle), installe leur GitHub App et donne-lui accès au repo.
3. **New project** → sélectionne le repo. AtlasFlow détecte le `Dockerfile` à la racine tout seul.
4. Ajoute les variables d'environnement :

   | Variable | Valeur |
   |---|---|
   | `DISCORD_TOKEN` | le token du bot |
   | `CHANNEL_ID` | l'ID du salon |
   | `SECRET` | le mot de passe partagé avec les overlays |

5. Crée le projet → le premier déploiement part tout seul. Vérifie dans les logs : `Bot Discord connecté en tant que …`.
6. Récupère l'URL du projet (`https://ton-projet.atlasflow.dev`). Dans le `config.toml` des overlays :

   ```toml
   server = "wss://ton-projet.atlasflow.dev/ws"
   ```

   (`wss://` et pas `ws://` — la connexion est chiffrée par AtlasFlow, et pas de port à préciser.)

Chaque `git push` redéploie automatiquement le serveur.

### Option B — chez toi

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

```powershell
cargo build --release -p livechat-overlay
```

Distribue `target\release\livechat-overlay.exe` (un seul fichier). Chaque pote :

1. Le met dans un dossier accessible en écriture (pas `C:\Program Files`) et le lance une première fois → un `config.toml` modèle est créé à côté de l'exe ;
2. Édite ce `config.toml` :
   ```toml
   server = "ws://IP_DU_SERVEUR:9000/ws"
   secret = "le-meme-mot-de-passe-que-le-serveur"
   ```
3. Relance l'overlay. Une icône apparaît dans la zone de notification (à côté de l'horloge) :
   - **Tester l'affichage** → affiche un média de test pour vérifier que l'overlay marche ;
   - **Quitter** → ferme l'overlay.

Un badge « 🟢 Connecté » s'affiche en haut à droite quand la connexion au serveur est établie. Ensuite, tout média posté dans le salon s'affiche chez tout le monde, puis disparaît.

Options du `config.toml` de l'overlay : durée d'affichage, volume des vidéos, position à l'écran (`center`, `top-left`, `top-right`, `bottom-left`, `bottom-right`), taille maximale.

> L'overlay nécessite WebView2, préinstallé sur Windows 11 (et sur Windows 10 récent).

## Compiler depuis zéro

Prérequis (Windows) :
- [Rust](https://rustup.rs) ;
- Visual Studio Build Tools 2022 avec la charge de travail C++ (`winget install Microsoft.VisualStudio.2022.BuildTools`).

Puis `cargo build --release` à la racine compile les deux programmes.

## Limites connues

- **Plein écran exclusif** : l'overlay s'affiche par-dessus les jeux en *fenêtré sans bordure* (borderless) — le mode par défaut de la plupart des jeux récents — mais **pas** en *plein écran exclusif*. C'est une limite de Windows (même l'overlay Discord officiel ne le fait qu'en s'injectant dans le jeu). Si l'overlay n'apparaît pas : passe le jeu en « Fenêtré sans bordure ».
- **Liens Discord temporaires** : les URLs des pièces jointes Discord expirent après ~24 h. Aucun impact en direct (les overlays affichent le média à la seconde où il est posté), mais un overlay lancé plus tard ne « rattrape » pas les anciens médias.
- **Types supportés** : images (png, jpg, webp, gif, bmp, avif) et vidéos (mp4, webm, mov, m4v), en pièce jointe ou en lien direct `https://` collé dans le message (les liens `http://` sont ignorés). Les GIF Tenor/Giphy intégrés (choisis via le sélecteur GIF de Discord) ne sont pas encore gérés.
- **Sécurité** : la connexion est en `ws://` (non chiffrée) avec un mot de passe partagé — suffisant entre potes. Pour du chiffrement, mets un reverse proxy TLS (Caddy, nginx) devant le serveur et utilise `wss://`.

## Dépannage

| Problème | Cause probable |
|---|---|
| `disallowed intents` au démarrage du serveur | **Message Content Intent** pas activé dans le portail développeur (étape 1.2) |
| Le bot ne réagit pas 📺 aux messages | Mauvais `channel_id`, ou le bot n'a pas accès au salon |
| L'overlay affiche « Connexion impossible : … 401 Unauthorized » | Le `secret` de l'overlay ne correspond pas à celui du serveur (`SECRET` sur AtlasFlow) |
| L'overlay affiche « Connexion impossible » (autre erreur) | Mauvaise adresse `server`, port pas redirigé, ou pare-feu |
| Le serveur refuse de démarrer : « secret… change-moi » | Choisis un vrai mot de passe dans `secret`/`SECRET` — la valeur d'exemple est refusée |
| L'overlay affiche « Déconnecté » puis se reconnecte | Normal après une coupure réseau — la reconnexion est automatique |
| Rien ne s'affiche en jeu | Jeu en plein écran exclusif → passe en fenêtré sans bordure |
| La vidéo n'a pas de son | Monte `volume` dans le config.toml de l'overlay (0.0 = muet) |
