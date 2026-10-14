# PROPOSAL: Potluck - Rooms for Shared Local Inference

IRC for AI: invite-only rooms where people chat and share their local models. The router finds members, punches through NAT, encrypts, and sends each request to a member who can serve it.

## Problem

- People run local models (llama.cpp, vLLM, Ollama) on their own GPUs; friends and teams can't easily use each other's.
- Sharing one means network setup, per-person credentials, and no way to see whose GPU is online, idle, and running which model.
- Hosted services hold every prompt and decide who gets access.

## Principles

- **Rooms are the trust boundary.** Members trust each other; only members see chat, models, or traffic.
- **No server required.** Peer-to-peer; any member can relay.
- **Owners stay in control.** Whoever shares a GPU sets its limits and can stop at any time.
- **Built for small rooms.** 2-30 people; every mechanism is sized for that.

## Concepts

| Concept | Definition |
|---|---|
| Identity | ed25519 keypair = libp2p peer ID. Nickname is display only. |
| Room | ID = hash(genesis). Genesis names the owner key. Has a member set, chat, and shares. |
| Member set | grow-only set of signed events, merged by union, order-free. Members = owner + joins carrying an owner-signed invite, minus joins named by a `remove`. |
| Invite | owner-signed `{room_id, invitee_key?, uses, expiry}` plus bootstrap addresses, shared as a ticket string. Not published; it travels inside the joins that use it. |
| Join | invitee-signed `{invite, nick}`. The admitting member checks expiry, `uses`, and `revoke`, and refuses removed keys unless the invite names them. Concurrent admissions may overshoot `uses`; the owner removes extras. |
| Remove | owner-signed `{join_id}`. Members drop the removed key's connections and refuse its streams. |
| Share | member-signed `{seq, model, ctx_len, max_concurrent, rate_per_member, visibility}`; latest `seq` wins. `model` is the backend's model name (`llama3.1:70b-q4_K_M`). |
| Heartbeat | each provider broadcasts live shares and queue depth every 15s; silent for 45s = offline. |

Ceiling: losing the owner key freezes membership; members start a new room.

## Flow

```
owner /invite bob ──► invite link ──► bob dials bootstrap peers, signs join, members accept
bob: /ask llama3.1:70b "explain this diff"
  bob's router ──► picks carol (online, shares the model, shortest queue)
  bob ══Noise══ [relay if NAT] ══ carol's node ──► localhost llama.cpp
  tokens stream back to bob only (/ask -r posts question and answer to the room)
```

## Security

- **Transport:** every member pair talks over libp2p Noise sessions bound to member keys; relays see only ciphertext.
- **Chat:** each message is signed by its sender and travels only over Noise streams between current members; nodes refuse room streams from non-members, so a removed member receives nothing new.
- **Discovery:** tickets carry bootstrap addresses; members exchange known addresses (relay circuits included) on every sync and redial them. Nothing about a room is published.
- **Provider sandbox:** the node talks only to the localhost inference server, opens no inbound ports, forwards only chat/completion calls for shared models, and never proxies admin APIs (pull, delete, load).
- **Disclosure:** a provider sees the plaintext prompts it serves; the client shows who served each answer.

## Router

| Job | How |
|---|---|
| Connectivity | rust-libp2p: QUIC/TCP, AutoNAT, DCUtR hole punching, circuit relay v2 through members |
| Membership | exchange member-set events and addresses on connect and on change; union and recompute |
| Chat | send to connected members; each member forwards new messages to its other members, deduplicated by message hash |
| Resolution | model name → members whose heartbeat lists it |
| Selection | shortest queue from the last heartbeat, then RTT, preferring pinned members; on error or drop, retry the next provider |
| History | none: members see messages while online |

## Owner Controls

- Per-share limits: max context, max tokens, concurrency, per-member rate, active hours.
- Local-first: the owner's own jobs preempt room jobs; shares pause under load.
- Visibility: whole room or chosen members; restricted shares are advertised only to them. One node can share into several rooms.
- Local request log `{member, model, tokens, ts}` and per-member block, so owners see and control who uses their GPU.
- `/unshare` takes effect immediately; in-flight requests get a clean error.

## Interfaces

- **TUI:** `/join /invite /revoke /remove /share /unshare /models /ask /who /pin /block /token`. `/models` lists each model with online providers.
- **Local OpenAI-compatible endpoint:** `http://127.0.0.1:7777/r/<room>/v1`, so apps and IDEs use room models directly. Requires a per-app bearer token (`/token`); rejects browser `Origin` and non-local `Host` headers. Returns 503 `no provider online` when no member serves the model.

## Threat Model

| Threat | Mitigation |
|---|---|
| Outsider reads chat or prompts | Noise per pair; room streams only between members; nothing published |
| Removed member keeps access | members disconnect it and refuse its streams |
| Leaked invite | expiring, limited-use, optionally invitee-bound; owner revokes |
| Member abuses a GPU | provider limits, request log, local block, owner removes |
| Provider snoops prompts | disclosed; client shows who served; requester pins or blocks providers |
| Provider misreports its model | room trust; requester pins trusted members |
| Requester hits backend admin APIs | provider forwards only chat/completion calls for shared models |
| Local process or website uses the endpoint | 127.0.0.1 bind, per-app token, `Origin`/`Host` checks (blocks CSRF, DNS rebinding) |
| Malicious relay | sees only ciphertext; router rotates relays that drop traffic |

## Stack

- Rust: rust-libp2p (transport, NAT, relay, DHT), tokio, axum (endpoint), ratatui (TUI).
- JSON wire messages and per-room JSON files for the member set and known addresses.
- Inference adapters: any OpenAI-compatible HTTP backend (llama.cpp server, vLLM, Ollama, LM Studio).

## Roadmap

| Phase | Deliverable | Exit criteria |
|---|---|---|
| 0 | Two peers across NAT; `/ask` routed to the other's llama.cpp over Noise | streamed completion via relay |
| 1 | Rooms: invites, joins, remove, signed chat, address exchange | 5 people chat; removed member is locked out |
| 2 | Shares, heartbeats, selection, limits, OpenAI endpoint, TUI | IDE uses a room model; public alpha |

## Later

Each item ships when its trigger shows up in real use.

| Feature | Trigger |
|---|---|
| Ops and co-owners (needs causal event ordering) | one owner can't keep up with invites and removals |
| MLS group encryption | rooms grow past ~50 members |
| Model verification | rooms form among people who don't know each other |
| Always-on anchors, history backfill, sender-key encryption at rest | members want history from while they were offline |
| DHT rendezvous | members' addresses change faster than they sync |
| Cross-room sharing | providers ask to serve more than one room's members from a single share |
| Usage reciprocity | providers report free-riding |
| Bots, sub-channels | rooms get busy enough to need them |
