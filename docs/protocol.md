# Protocol

Both protocols run over libp2p streams between room members (Noise-encrypted, Yamux-multiplexed).

## `/potluck/room/1`

One JSON message per stream, sender half-closes, optional JSON reply:

| Message | Reply |
|---|---|
| `join` (signed join event) | `sync` with the room's events, or `denied` |
| `sync {events, peers}` from a member | `sync` with the receiver's events and peers, or `denied` |
| `cast` (signed chat or heartbeat, from a member) | none; receiver forwards new ones to its other members |

## `/potluck/ask/1`

- Requester sends one OpenAI chat-completions JSON body, then half-closes.
- Provider replies with a line `{"status":<http status>,"error"?:<msg>}`, then the backend's body verbatim.
- Provider refuses non-members, blocked members, unshared models (404), and requests over its limits (429, so the router tries the next provider); it forwards only to `<backend>/v1/chat/completions`, with `max_tokens` clamped.
- After `/unshare`, an in-flight stream ends with `data: {"error":...}`.

## State

In `~/.config/potluck/` (override with `--dir`):

| File | Contents |
|---|---|
| `key` | node's ed25519 key |
| `rooms/<id>.json` | events, known peers, shares, endpoint tokens |
| `requests.log` | provider request log `{ts, member, model, status}` |
| `invite.txt` | last ticket from `/invite` |
