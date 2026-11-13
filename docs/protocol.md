# Protocol

Both protocols run over libp2p streams between room members (Noise-encrypted, Yamux-multiplexed).

## `/potluck/room/1`

One JSON message per stream, sender half-closes, optional JSON reply:

| Message | Reply |
|---|---|
| `join` (signed join event) | `sync` with the room's events, or `denied`; a repeat from an admitted member gets `sync` again |
| `sync {events, peers}` from a member | `sync` with the receiver's events and peers, or `denied` |
| `cast` (signed chat or heartbeat, from a member) | none; receiver forwards new ones to its other members |

## `/potluck/ask/1`

- Requester sends one OpenAI chat-completions JSON body, then half-closes.
- Provider replies with a line `{"status":<http status>,"error"?:<msg>}`, then the backend's body verbatim.
- Provider refuses non-members, blocked members, unshared models (404), and requests over its limits (429); it forwards only to `<backend>/v1/chat/completions`, with `max_completion_tokens`, `n` and `n_predict` removed and `max_tokens` clamped to the share's cap, if it has one.
- Router: 403, 404, 429, 502, and a backend's own 429 or 503, make it try the next provider; a provider with no reply within 10 minutes is skipped; if all refuse, the caller gets 503 with the last reason. 400 goes back as is. A body of 1 MiB or more gets 413 from the requester's own node before any provider is contacted.
- After `/unshare`, an in-flight stream ends with `data: {"error":...}`.

## State

In `~/.config/potluck/` (override with `--dir`), mode 0700; room files are written atomically:

| File | Contents |
|---|---|
| `key` | node's ed25519 key |
| `rooms/<id>.json` | events, known peers, shares, pins, blocks, endpoint tokens; kept when rejoining |
| `requests.log` | provider request log `{ts, member, model, status}`; model name cut to 128 characters |
| `invite.txt` | last ticket from `/invite` |
