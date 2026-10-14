# PROPOSAL: Potluck - Rooms for Shared Local Inference

IRC for AI: invite-only rooms where people chat and share their local models freely. The router finds members, punches through NAT, encrypts, and sends each request to a member who can serve it.

## Problem

- People run local models (llama.cpp, vLLM, Ollama) on their own GPUs; friends, teams, and communities can't easily use each other's.
- Exposing an inference server means open ports, tunnels, auth, and API keys handed around by hand.
- Hosted alternatives centralize prompts and logs and decide who gets access.

## Principles

- **Rooms are the trust boundary.** Only invited members can see the chat, models, or traffic.
- **No server required.** Peer-to-peer; any member can relay. An always-on node is optional and has no special authority.
- **Owners stay in control.** Whoever shares a GPU sets its limits and can stop sharing at any time.

## Concepts

| Concept | Definition |
|---|---|
| Identity | ed25519 keypair = libp2p peer ID. Nickname is display only; the key is identity. |
| Room | invite-only group: ID = hash(genesis event). Has a member log, chat, and shared models. |
| Member log | signed, append-only event log (genesis, invite, join, kick, role, mode). Every member replays it to compute membership. |
| Roles | `owner` > `op` > `member` (IRC `~ @ +`). Ops invite and kick; modes set who can share or invoke. |
| Invite | signed capability `{room_id, issuer, invitee_key?, uses, expiry, role, bootstrap_peers}`, shared as link or QR. |
| Share | a member's advertisement in the room: `{model_id, ctx_len, max_concurrent, quota_per_member, visibility}` |
| Model ID | sha256 of the weights file + quant. The display name (`llama3-70b`) is an alias that resolves to a hash. |
| Anchor | optional always-on member (like an IRC bouncer) that keeps history and relays. Holds no extra rights. |

## Flow

```
alice /invite bob ──► invite link ──► bob joins: dials bootstrap peers, proves invite, MLS adds bob
bob: /ask @llama3-70b "explain this diff"
  bob's router ──► picks carol (shares llama3-70b, idle, low RTT)
  bob ══Noise E2E══ [relay if NAT] ══ carol's node ──► localhost llama.cpp
  tokens stream back ──► posted to room chat (or private if /ask -p)
```

## Security

- **Group encryption:** MLS (RFC 9420) for room chat and control messages. Forward secrecy; keys rotate on every join and kick, so a kicked member can't read anything newer.
- **Inference sessions:** separate 1:1 Noise channel between requester and provider, bound to their member keys. Relays only see ciphertext.
- **Joining:** the invitee proves the invite signature plus possession of their key; members check it against the member log before MLS adds them.
- **Discovery privacy:** no public model listings. Members find each other at DHT rendezvous keys `H(room_secret, epoch)`; outsiders can't tell a room exists.
- **Provider sandbox:** the node talks only to the localhost inference server, never exposes it, and opens no inbound ports.
- **Honest disclosure:** a provider sees the plaintext prompts it serves. The room is the trust model; the client shows who served each answer.

## Router

The router is the part of each client that "takes care of the rest":

| Job | How |
|---|---|
| Connectivity | libp2p: QUIC/TCP, AutoNAT, DCUtR hole punching, circuit relay v2 through members or anchors |
| Membership | sync the member log + MLS state on connect; gossipsub topic per room for chat and shares |
| Model resolution | alias → model IDs → members currently sharing them |
| Selection | pick a provider by load, RTT, and remaining quota; fail over mid-queue if a provider drops |
| Fairness | each provider runs a fair queue per member plus its own quotas. No accounting, just politeness. |
| History | members keep encrypted history and new joiners backfill from peers. Only events after joining are readable. |

## Owner Controls

- Per-share limits: max context, max tokens, concurrency, per-member rate, active hours.
- Local-first: the owner's own jobs preempt room jobs, and shares pause automatically under load.
- Visibility: share with a whole room, chosen roles, or chosen members. One node can share into several rooms.
- `/unshare` takes effect immediately; in-flight requests get a clean error.

## Interfaces

- **TUI chat client:** IRC-style commands `/join /invite /kick /op /share /unshare /models /ask /who`.
- **Local OpenAI-compatible endpoint:** `http://localhost:7777/r/<room>/v1`, so existing apps and IDEs use room models transparently.
- **Bots:** a share can be addressed in chat (`@llama3-70b ...`) and answers appear as messages from the serving member.

## Threat Model

| Threat | Mitigation |
|---|---|
| Outsider reads chat or prompts | MLS group encryption, Noise per session, rendezvous keys derived from the room secret |
| Kicked member keeps access | MLS epoch rotation on kick; new rendezvous keys |
| Leaked invite | single-use or expiring invites; invitee-bound when the key is known; ops can revoke |
| Member abuses a GPU | provider quotas, per-member rate limits, ops kick |
| Rogue op | roles are ordered; only the owner can demote ops; log is signed and auditable |
| Provider snoops prompts | disclosed; requester chooses the provider or pins trusted members |
| Malicious relay | sees only ciphertext; router rotates relays that drop traffic |

## Stack

- Go + go-libp2p for node, router, relay; MLS via an existing library (e.g. OpenMLS over FFI, or a Go port).
- Protobuf for wire messages; SQLite for the local log, history, and MLS state.
- Inference adapters: any OpenAI-compatible HTTP backend (llama.cpp server, vLLM, Ollama, LM Studio).

## Roadmap

| Phase | Deliverable | Exit criteria |
|---|---|---|
| 0 | Two peers across NAT; `/ask` routed to the other's llama.cpp over Noise | streamed completion via relay |
| 1 | Rooms: member log, invites, roles, gossip chat (no MLS yet) | 5 people chat and share in one room |
| 2 | MLS group encryption, kick rotation, rendezvous discovery | kicked member is locked out |
| 3 | Router selection, fair queues, quotas, OpenAI endpoint | IDE uses a room model transparently |
| 4 | Anchors, history backfill, TUI polish, multi-room | public alpha |

## Open Questions

- Should a room's member log allow forks (split rooms) or resolve concurrent op actions by owner priority?
- Large rooms: MLS scales well, but should model gossip be summarized above ~500 members?
- Federation: can rooms bridge (like IRC networks), or does each room stay sovereign?
- Do we need optional reciprocity hints ("gave/used tokens") as social signals, without any currency?
