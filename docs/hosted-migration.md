# Hosted migration

Keep the public repo self-hostable. Put hosted-only code in a private repo.

## Public repo

- `server/`: self-host server.
- `overlay/`: Windows overlay.
- `docs/protocol.md`: protocol both servers must respect.
- `docker-compose.yml`: easy self-host deploy.

## Private hosted repo

Minimum hosted backend:

- official Discord bot;
- DB table for guild config;
- Discord OAuth for overlay login;
- Stripe webhook that toggles subscription state;
- WebSocket endpoint compatible with `docs/protocol.md`.

Minimal data model:

```txt
guilds(
  guild_id,
  name,
  channel_id,
  subscription_active,
  created_at,
  updated_at
)

sessions(
  token_hash,
  discord_user_id,
  expires_at
)
```

Start with one configured channel per guild. Add multiple channels only when users ask for it.
