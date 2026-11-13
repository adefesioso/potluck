# Protocol

Potluck has two protocols. Both run over libp2p streams between room members, which are encrypted with Noise and multiplexed with Yamux.

## `/potluck/room/1`

Each stream carries one JSON message. The sender writes it and half-closes, and the receiver may write one JSON reply. A node reads at most 4 MiB from a member and 64 KiB from anyone else, and gives up after 30 seconds.

| Message | Reply |
|---|---|
| `join`, carrying a signed join event | `sync` with the room's events and addresses, or `denied` with a reason. An admitted member who sends the same join again gets `sync` again, in case the first reply was lost. |
| `sync`, carrying events and peer addresses, from a member | `sync` with the receiver's own events and addresses, or `denied`. |
| `cast`, carrying a signed chat message or heartbeat, from a member | None. The receiver forwards the ones it has not seen to its other connected members. |

A non-member may only send `join`. Anything else is answered with `denied`.

## `/potluck/ask/1`

The requester sends one OpenAI chat-completions JSON body and half-closes. The provider answers with a single line, `{"status":<http status>,"error"?:<message>}`, followed by the backend's response body unchanged.

The provider refuses a request before it reaches the backend in these cases:

| Status | Reason |
|---|---|
| 403 | The requester is not a member, or the provider has blocked them. |
| 404 | The model is not shared here. |
| 429 | The requester is over the share's per-minute limit, or the share is at its concurrency limit. |
| 400 | The body is not a JSON object, or did not arrive within 30 seconds. |
| 413 | The body is 1 MiB or more. |
| 502 | The backend cannot be reached. |

Accepted requests are forwarded only to `<backend>/v1/chat/completions`. If the share has a token cap, the provider sets `max_tokens` to the smaller of the cap and what was asked, and removes `max_completion_tokens`, `n` and `n_predict` so that no other field can get around it.

On the requester's side, the router treats a refusal with status 403, 404, 429 or 502 as a reason to try the next provider, and does the same when a backend itself answers 429 or 503. A provider that sends no reply within 10 minutes is skipped. If every provider fails, the caller gets 503 with the last reason. A 400 goes back to the caller as is. A body of 1 MiB or more never leaves the requester's node, which answers 413 itself.

If the model is unshared while a stream is in flight, the stream ends with `data: {"error":{"message":"model unshared"}}`. A backend failure mid-stream ends it the same way, with the message `backend error`. A provider also drops a stream when the requester stops reading for 60 seconds.

## State

State lives in `~/.config/potluck/`, or wherever `--dir` points. The directory is private to the user (mode 0700), and room files are written atomically.

| File | Contents |
|---|---|
| `key` | The node's ed25519 key, created on first run. |
| `rooms/<id>.json` | The room's events and known peer addresses, plus this node's own shares, pins, blocks and endpoint tokens. The local settings are kept when you rejoin a room. |
| `requests.log` | One line per request served as a provider: time, member, model, status. The model name is cut to 128 characters. |
| `invite.txt` | The last ticket written by `/invite`. |
