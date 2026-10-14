# Potluck

Your GPU idles most of the day. So do your friends'. Potluck pools them: invite-only rooms where you chat and share local models peer-to-peer, through any NAT, encrypted end to end. No server, no API keys handed around, no one else holding your prompts. Every app in the room gets one OpenAI-compatible endpoint backed by everyone's hardware.

**Bring a model, pull up a chair:** build it, `potluck create`, send an invite. Design and roadmap: [PROPOSAL.md](PROPOSAL.md).

## Problem

Friends and teams run local models (llama.cpp, vLLM, Ollama) but can't easily use each other's: sharing needs network setup, per-person credentials, and a way to find whose GPU is free.

## How it works

| Piece | Role |
|---|---|
| Node | one `potluck` binary per person; ed25519 key = libp2p peer ID = identity |
| Room | owner-created, invite-only; membership = signed events every member merges ([concepts](PROPOSAL.md#concepts)) |
| Relay | any reachable node; forwards encrypted circuits for peers behind NAT, then DCUtR tries a direct hole-punched path |
| Provider | member that `/share`s a model; forwards room requests to its localhost OpenAI-compatible backend within its limits |
| Router | picks a provider from heartbeats, opens a Noise-encrypted stream, streams the answer back |
| Endpoint | local OpenAI-compatible API, so apps and IDEs use room models |

## Status

Phase 2 of the [roadmap](PROPOSAL.md#roadmap): rooms with invites, removal, chat, model sharing, and the local endpoint. Public alpha.

## Usage

```sh
cargo build --release
potluck relay --port 4001                    # on a reachable host; prints: listening <addr>
potluck create friends --nick alice          # prints: room <id>
potluck chat friends                         # chat; /invite saves a ticket to invite.txt
potluck join <ticket> --nick bob             # joins, then chats
```

`chat` and `join` open a full-screen TUI on a terminal (PageUp/PageDown scroll, Up/Down history, Ctrl-C quits) and plain stdin/stdout lines when piped. Both take `--relay <relay addr>` (for nodes behind NAT) and `--api <addr>` (endpoint, default `127.0.0.1:7777`).

| Command | Effect |
|---|---|
| `/invite [uses] [hours] [peer-id]`, `/revoke <id>`, `/remove <nick>` | owner only: manage members |
| `/who` | members and who's online |
| `/share <model> <backend-url> [max_concurrent] [per_min] [max_tokens]`, `/unshare <model>` | serve a local model to the room (defaults 1, 30, none) |
| `/models`, `/ask <model> <prompt>` | list online models; ask one |
| `/pin <nick>`, `/block <nick>` | toggle: prefer a provider; refuse to use or serve a member |
| `/token <app>` | bearer token for the endpoint |
| other lines | chat messages |

Endpoint: `http://127.0.0.1:7777/r/<room>/v1` with `Authorization: Bearer <token>`. Point any OpenAI-compatible client there.

State lives in `~/.config/potluck/` (override with `--dir`): `key`, `rooms/<id>.json` (events, peers, shares, tokens), `requests.log`, `invite.txt` (last ticket).

## Wire protocol `/potluck/ask/1`

- Requester sends one OpenAI chat-completions JSON body, then half-closes.
- Provider replies with a line `{"status":<http status>,"error"?:<msg>}`, then the backend's body verbatim.
- Provider refuses non-members, blocked members, unshared models (404), and requests over its limits (429, so the router tries the next provider); it forwards only to `<backend>/v1/chat/completions`, with `max_tokens` clamped.
- After `/unshare`, an in-flight stream ends with `data: {"error":...}`.

## Wire protocol `/potluck/room/1`

One JSON message per stream, sender half-closes, optional JSON reply:

| Message | Reply |
|---|---|
| `join` (signed join event) | `sync` with the room's events, or `denied` |
| `sync {events, peers}` from a member | `sync` with the receiver's events and peers, or `denied` |
| `cast` (signed chat or heartbeat, from a member) | none; receiver forwards new ones to its other members |

## Layout

| File | Contents |
|---|---|
| `src/main.rs` | CLI: `relay`, `create`, `chat`, `join` |
| `src/net.rs` | key loading and swarm setup (TCP/QUIC, Noise, Yamux, relay, DCUtR) |
| `src/room.rs` | signed events, membership, admission checks, tickets |
| `src/node.rs` | room node: swarm loop, `/potluck/room/1` handlers, chat REPL |
| `src/node/infer.rs` | provider, router, heartbeats, endpoint |
| `src/ui.rs` | output sink; line frontend and ratatui TUI |
| `tests/phase1.rs` | five members chat; a removed member is locked out |
| `tests/phase2.rs` | an OpenAI client uses a model shared by another member |
