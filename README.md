# Potluck

Invite-only rooms where people chat and share their local models. Peer-to-peer, NAT-traversing, end-to-end encrypted. Design and roadmap: [PROPOSAL.md](PROPOSAL.md).

## Problem

Friends and teams run local models (llama.cpp, vLLM, Ollama) but can't easily use each other's: sharing needs network setup, per-person credentials, and a way to find whose GPU is free.

## How it works

| Piece | Role |
|---|---|
| Node | one `potluck` binary per person; ed25519 key = libp2p peer ID = identity |
| Room | owner-created, invite-only; membership = signed events every member merges ([concepts](PROPOSAL.md#concepts)) |
| Relay | any reachable node; forwards encrypted circuits for peers behind NAT, then DCUtR tries a direct hole-punched path |
| Provider | node that serves requests by forwarding them to its localhost OpenAI-compatible backend |
| Requester | node that opens a Noise-encrypted stream to a provider and streams tokens back |

## Status

Phase 1 of the [roadmap](PROPOSAL.md#roadmap): rooms with invites, removal, and chat. Model sharing (`serve`/`ask`) is not yet tied to rooms: anyone who knows a provider's address can use it.

## Usage

```sh
cargo build --release
# on a reachable host
potluck relay --port 4001                    # prints: listening /ip4/.../p2p/<relay-id>
# on the GPU box, next to llama.cpp / Ollama
potluck serve --relay /ip4/.../p2p/<relay-id> --backend http://127.0.0.1:8080 --model llama3
                                             # prints: serving <relay>/p2p-circuit/p2p/<provider-id>
# anywhere
potluck ask --peer <serving address> --model llama3 "explain this diff"

# rooms (add --relay <relay addr> on nodes behind NAT)
potluck create friends --nick alice          # prints: room <id>
potluck chat friends                         # line-based chat; /invite prints a ticket
potluck join <ticket> --nick bob             # joins, then chats
```

Chat commands: `/invite [uses] [hours] [peer-id]`, `/revoke <invite-id>`, `/remove <nick>`, `/who`, `/quit`; other lines are messages. Only the owner invites, revokes, and removes.

State lives in `~/.config/potluck/` (override with `--dir`): `key`, `rooms/<id>.json`.

## Wire protocol `/potluck/ask/0`

- Requester sends one OpenAI chat-completions JSON body, then closes its write half.
- Provider forwards it to `<backend>/v1/chat/completions` with `stream: true`, only if `model` matches `--model`, and relays the SSE bytes back verbatim.
- Provider-side failures arrive as one SSE event `data: {"error":{"message":...}}`.

## Wire protocol `/potluck/room/1`

One JSON message per stream, sender half-closes, optional JSON reply:

| Message | Reply |
|---|---|
| `join` (signed join event) | `sync` with the room's events, or `denied` |
| `sync {events, peers}` from a member | `sync` with the receiver's events and peers, or `denied` |
| `chat` (signed, from a member) | none; receiver forwards new messages to its other members |

## Layout

| File | Contents |
|---|---|
| `src/main.rs` | CLI; `relay`, `serve`, `ask`, and room lookup for `create`, `chat`, `join` |
| `src/net.rs` | key loading and swarm setup (TCP/QUIC, Noise, Yamux, relay, DCUtR) |
| `src/room.rs` | signed events, membership, admission checks, tickets |
| `src/node.rs` | room node: swarm loop, `/potluck/room/1` handlers, chat REPL |
| `tests/phase0.rs` | relay + provider + requester against a fake backend on localhost |
| `tests/phase1.rs` | five members chat; a removed member is locked out |
