# Development

## Stack

- Rust: rust-libp2p (TCP/QUIC, Noise, Yamux, relay, DCUtR), tokio, axum (endpoint), ratatui (TUI).
- JSON wire messages and per-room JSON files ([protocol.md](protocol.md)).
- Inference backends: any OpenAI-compatible HTTP server (llama.cpp server, vLLM, Ollama, LM Studio).

## Layout

| File | Contents |
|---|---|
| `src/main.rs` | CLI: `relay`, `create`, `chat`, `join` |
| `src/net.rs` | key loading and swarm setup (TCP/QUIC, Noise, Yamux, relay, DCUtR) |
| `src/room.rs` | signed events, membership, admission checks, tickets |
| `src/node.rs` | room node: swarm loop, `/potluck/room/1` handlers, chat REPL |
| `src/node/infer.rs` | provider, router, heartbeats, endpoint |
| `src/ui.rs` | output sink; line frontend and ratatui TUI |

## Tests

| Test | Checks |
|---|---|
| `tests/phase1.rs` | five members chat; a removed member is locked out |
| `tests/phase2.rs` | an OpenAI client uses a member's model through the endpoint |
| `tests/lab.sh` | on a pvelab Proxmox node: two members behind separate NATs join, chat, and share via relay |
