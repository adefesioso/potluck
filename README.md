# Potluck

Invite-only rooms where people chat and share their local models. Peer-to-peer, NAT-traversing, end-to-end encrypted. Design and roadmap: [PROPOSAL.md](PROPOSAL.md).

## Problem

Friends and teams run local models (llama.cpp, vLLM, Ollama) but can't easily use each other's: sharing needs network setup, per-person credentials, and a way to find whose GPU is free.

## How it works

| Piece | Role |
|---|---|
| Node | one `potluck` binary per person; ed25519 key = libp2p peer ID = identity |
| Relay | any reachable node; forwards encrypted circuits for peers behind NAT, then DCUtR tries a direct hole-punched path |
| Provider | node that serves requests by forwarding them to its localhost OpenAI-compatible backend |
| Requester | node that opens a Noise-encrypted stream to a provider and streams tokens back |

## Status

Phase 0 of the [roadmap](PROPOSAL.md#roadmap): two peers across NAT, `ask` routed to the other's backend over Noise, streamed via relay. No rooms yet: anyone who knows a provider's address can use it.

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
```

Keys live at `~/.config/potluck/key` (override with `--key`).

## Wire protocol `/potluck/ask/0`

- Requester sends one OpenAI chat-completions JSON body, then closes its write half.
- Provider forwards it to `<backend>/v1/chat/completions` with `stream: true`, only if `model` matches `--model`, and relays the SSE bytes back verbatim.
- Provider-side failures arrive as one SSE event `data: {"error":{"message":...}}`.

## Layout

| File | Contents |
|---|---|
| `src/main.rs` | CLI and the `relay`, `serve`, `ask` commands |
| `src/net.rs` | key loading and swarm setup (TCP/QUIC, Noise, Yamux, relay, DCUtR) |
| `tests/phase0.rs` | relay + provider + requester against a fake backend on localhost |
