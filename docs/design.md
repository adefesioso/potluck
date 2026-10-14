# Design

IRC for AI: rooms where people chat and share local models; the router finds members, punches through NAT, encrypts, and sends each request to a member who can serve it.

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
| Share | provider-local config `{model, backend, max_concurrent, rate_per_min, max_tokens}`. `model` is the backend's model name (`llama3.1:70b-q4_K_M`). |
| Heartbeat | member-signed `{ts, models, busy}` flooded like chat every 15s while sharing and on every change; latest `ts` wins; silent for 45s = offline. |

Ceiling: losing the owner key freezes membership; members start a new room.

## Flow

```
owner /invite bob ──► invite link ──► bob dials bootstrap peers, signs join, members accept
bob: /ask llama3.1:70b "explain this diff"
  bob's router ──► picks carol (online, shares the model, least busy)
  bob ══Noise══ [relay if NAT] ══ carol's node ──► localhost llama.cpp
  tokens stream back to bob only
```

## Router

| Job | How |
|---|---|
| Connectivity | rust-libp2p: QUIC/TCP, AutoNAT, DCUtR hole punching, circuit relay v2 through members |
| Membership | exchange member-set events and addresses on connect and on change; union and recompute |
| Chat | send to connected members; each member forwards new messages to its other members, deduplicated by message hash |
| Resolution | model name → members whose heartbeat lists it |
| Selection | pinned members first, then least busy per last heartbeat; on busy, refusal, or unreachable, try the next provider |
| History | none: members see messages while online |

## Owner controls

- Per-share limits: concurrency, per-member requests per minute, max tokens per request.
- Local request log `{ts, member, model, status}` and per-member `/block`, so owners see and control who uses their GPU.
- `/unshare` takes effect immediately: new requests are refused, in-flight streams end with an error event.
