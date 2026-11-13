# Development

## Stack

Potluck is written in Rust 1.88 or newer and builds on Linux and macOS only. Networking is rust-libp2p with TCP and QUIC transports, Noise, Yamux, circuit relay and DCUtR. The runtime is tokio, the local endpoint is axum, and the TUI is ratatui.

Wire messages and room files are JSON, as described in [protocol.md](protocol.md).

An inference backend is any HTTP server that speaks the OpenAI chat-completions API, such as llama.cpp server, vLLM, Ollama or LM Studio.

## Layout

| File | Contents |
|---|---|
| `src/main.rs` | The CLI: `relay`, `create`, `chat` and `join`. |
| `src/net.rs` | Key loading and swarm setup. |
| `src/room.rs` | Signed events, membership, admission checks and tickets. |
| `src/node.rs` | The room node: the swarm loop, the `/potluck/room/1` handlers and the chat commands. |
| `src/node/infer.rs` | Provider, router, heartbeats and the local endpoint, including `/potluck/ask/1`. |
| `src/ui.rs` | The output sink and the two frontends, plain lines and the ratatui TUI. |

## Tests

`cargo test` runs the unit tests and both integration tests. The integration tests start real nodes on localhost and drive them through the plain-line frontend.

| Test | What it checks |
|---|---|
| Unit tests in `src/room.rs` | Invites, joins, removal, nick rules and revocation. |
| Unit tests in `src/ui.rs` | Input editing, history and TUI rendering. |
| `tests/phase1.rs` | Five members chat, and a removed member is locked out. |
| `tests/phase2.rs` | An OpenAI client uses a model another member shares, through the local endpoint. |
| `tests/common/mod.rs` | The harness the two integration tests share. |
| `tests/lab.sh` | Run by hand on a pvelab Proxmox node. Two members behind separate NATs join, chat and share through a relay, with `tests/lab-backend.py` standing in for the model. |
