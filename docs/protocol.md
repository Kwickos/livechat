# LiveChat protocol

This is the public contract between the overlay and any compatible server.

## Self-host WebSocket

Self-host servers expose:

```txt
GET /ws?token=<shared-secret>
```

The connection is accepted when `token` matches the server `SECRET`.

## Hosted WebSocket

The hosted service should keep the same media messages, but authenticate with a user/session token:

```txt
GET /ws?guild_id=<discord-guild-id>&token=<session-token>
```

The hosted backend must check:

- the session token is valid;
- the Discord user can access the guild;
- the bot is still installed in the guild;
- the guild subscription is active.

## Server to overlay message

```json
{
  "type": "media",
  "kind": "image",
  "url": "https://cdn.discordapp.com/...",
  "filename": "image.png",
  "sender": "Alex",
  "caption": "optional message"
}
```

Fields:

| Field | Type | Notes |
|---|---|---|
| `type` | string | Always `media` for display events. |
| `kind` | string | `image`, `gif`, `video`, or `audio`. |
| `url` | string | Direct media URL reachable by the overlay. |
| `filename` | string | Optional display/file name. |
| `sender` | string | Discord display name. |
| `caption` | string | Optional message content. |

## Compatibility rule

Servers may add fields. The overlay must ignore unknown fields.
