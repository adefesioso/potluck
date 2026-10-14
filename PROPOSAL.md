# PROPOSAL: Potluck - Rooms for Shared Local Inference

IRC for AI: invite-only rooms where people chat and share their local models freely. The router finds members, punches through NAT, encrypts, and sends each request to a member who can serve it.

## Problem

- People run local models (llama.cpp, vLLM, Ollama) on their own GPUs; friends, teams, and communities can't easily use each other's.
- Exposing an inference server means open ports, tunnels, auth, and API keys handed around by hand.
- Hosted alternatives centralize prompts and logs and decide who gets access.

## Principles

- **Rooms are the trust boundary.** Only invited members can see the chat, models, or traffic.
- **No server required.** Peer-to-peer; any member can relay. An always-on node is optional and has no special authority.
- **Rooms stay sovereign.** No IRC-style network; rooms link only through bridges or grants, and only for shares whose owner opted in.
- **Owners stay in control.** Whoever shares a GPU sets its limits and can stop sharing at any time.

## Concepts

| Concept | Definition |
|---|---|
| Identity | ed25519 keypair = libp2p peer ID. Nickname is display only; the key is identity. |
| Room | invite-only group: ID = hash(genesis event). Has a member log, chat, and shared models. |
| Member log | signed event DAG (genesis, invite, revoke, join, kick, role, mode, recover, rollback). Every member replays it to compute membership. An event is valid only if its issuer held the role at its causal parents; a kick or demote voids every event by that key outside its causal past. Concurrent conflicts: higher role > removal beats grant > lower hash. Timestamps are display-only. |
| Roles | `owner` > `op` > `member` (IRC `~ @ +`). Ops invite and kick; modes set who can share or invoke. Issuers act only on lower roles (owners also on owners); only owners grant or remove op. After demoting or kicking an op, an owner may issue `rollback {op, since}`: every member that op kicked at or after `since` (default: the op grant) is restored at their prior role and re-added to MLS without an invite, unless a separate valid kick also removed them. Messages from the gap stay unreadable to them. Multiple owners allowed; if all owner keys are lost, a majority of ops files `recover`, effective after 7 days unless any owner signs an event. |
| Invite | signed capability `{room_id, issuer, invitee_key?, uses, expiry, role, bootstrap_peers}`, shared as link or QR. `role` must be below the issuer's. Dies when a `revoke` names it or the issuer loses the role. |
| Share | a member's signed record, latest by per-member sequence number (not clock): `{seq, model_id, ctx_len, max_concurrent, quota_per_member, visibility, bridgeable}` |
| Model ID | sha256 of the weights file + quant. The display name (`llama3-70b`) is an alias that resolves to a hash. A provider's claim until its fingerprint checks out. |
| Fingerprint | top-5 logprobs of the first 8 greedy tokens for 16 canary prompts. Any member holding the weights runs `/attest <model>` and publishes a signed `{model_id, fingerprint}`. Disagreeing attestations mark the model ID `disputed`. |
| Anchor | optional always-on member (like an IRC bouncer) that keeps history and relays. Holds no extra rights. |
| Fork | explicit split (`/fork`): new genesis references the parent room; members opt in. Never implicit. |
| Bridge | member of two rooms that re-shares `bridgeable` shares into the other room under its own identity and quota; sees forwarded plaintext. |
| Receipt | requester-signed `{provider, model_id, tokens, ts}` with the requester's own token count, checkpointed every 1k tokens (provider pauses until signed); provider keeps it and publishes it to the room under `+r`. |

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
- **Joining:** the invitee proves the invite signature plus possession of their key; members check it against the member log before MLS adds them. Clients reject any MLS commit not backed by a valid member-log event.
- **Discovery privacy:** no public model listings. Members find each other at DHT rendezvous keys `H(room_secret, day)`; `room_secret` rotates on kick, and members who missed a rotation reconnect via cached peers. Outsiders can't tell a room exists.
- **Provider sandbox:** the node talks only to the localhost inference server, never exposes it, and opens no inbound ports. It forwards only chat/completion calls for the shared model, enforces share limits itself, and never proxies backend admin APIs (pull, delete, load).
- **Honest disclosure:** a provider sees the plaintext prompts it serves. The room is the trust model; the client shows who served each answer.

## Router

The router is the part of each client that "takes care of the rest":

| Job | How |
|---|---|
| Connectivity | libp2p: QUIC/TCP, AutoNAT, DCUtR hole punching, circuit relay v2 through members or anchors |
| Membership | sync the member log + MLS state on connect; the winning branch sets the next MLS epoch, losing-branch clients re-send under it |
| Gossip | gossipsub topic per room for chat; shares sync by set reconciliation, gossip carries deltas only |
| Model resolution | alias → model IDs → members currently sharing them |
| Selection | probe 2–3 candidates for load (never gossiped); pick by load, RTT, and remaining quota, preferring pinned members; fail over mid-queue if a provider drops |
| Verification | router always asks for logprobs (stripped before the app sees them), so canary spot-checks (~1% of requests) look like ordinary traffic. Match = same top-1 tokens and logprob distance within a tolerance set per backend. A mismatch marks the share `mismatch` locally and drops it from auto-selection; `/models -v` shows `verified`, `unverified`, `mismatch`, or `disputed`. |
| Fairness | each provider runs a fair queue per member plus its own quotas; may locally weight toward members who served *it*, using only its own receipts. No currency. |
| History | members keep encrypted history and new joiners backfill from peers. Only events after joining are readable. |

## Owner Controls

- Per-share limits: max context, max tokens, concurrency, per-member rate, active hours.
- Local-first: the owner's own jobs preempt room jobs, and shares pause automatically under load.
- Visibility: share with a whole room, chosen roles, or chosen members; restricted shares are advertised only to eligible members. Bridging is off unless `bridgeable`. One node can share into several rooms.
- `/unshare` takes effect immediately; in-flight requests get a clean error.

## Interfaces

- **TUI chat client:** IRC-style commands `/join /invite /kick /op /fork /share /unshare /models /ask /attest /who`. `/models` aggregates per alias (`-v` lists providers); sub-channels `#room/<name>` split chat in large rooms.
- **Reciprocity mode `+r`:** off by default; `/who` shows rough served/used buckets from published receipts over a rolling 30 days. Social signal only; never affects routing.
- **Local OpenAI-compatible endpoint:** `http://127.0.0.1:7777/r/<room>/v1`, so existing apps and IDEs use room models transparently. Requires a per-app bearer token (`/token`); rejects browser `Origin` and non-local `Host` headers.
- **Bots:** a share can be addressed in chat (`@llama3-70b ...`) and answers appear as messages from the serving member. Bot output never triggers other bots.

## Threat Model

| Threat | Mitigation |
|---|---|
| Outsider reads chat or prompts | MLS group encryption, Noise per session, rendezvous keys derived from the room secret |
| Kicked member keeps access | MLS epoch rotation on kick; new rendezvous keys |
| Leaked invite | single-use or expiring invites; invitee-bound when the key is known; ops can revoke |
| Member abuses a GPU | provider quotas, per-member rate limits, ops kick |
| Rogue op | acts only on lower roles; only owners grant or remove op; demotion voids the op's concurrent events; owner `rollback` restores the members they kicked; `recover` has a 7-day owner veto |
| Backdated or replayed events | timestamps display-only; validity from DAG causality; revocation voids concurrent events |
| Provider snoops prompts | disclosed; client shows who served; requester pins or blocks providers |
| Provider fakes a model or quant | canary spot-checks against attested fingerprints; a provider that detects canaries and runs the real model only for them gets past this, so pin trusted members for sensitive work |
| False attestation to smear honest providers | attestations are signed; `disputed` shows every attester; a mismatch only affects the local router and is never broadcast |
| Requester hits backend admin APIs | provider forwards only chat/completion calls for the shared model |
| Local process or website uses the endpoint | 127.0.0.1 bind, per-app token, `Origin`/`Host` checks (blocks CSRF, DNS rebinding) |
| Free-riding or faked reciprocity | checkpointed receipts; providers weight only by their own receipts |
| Sybil alts multiply quotas | only ops invite; log records each inviter; providers can cap quota per inviter |
| Bridge or grant drains GPUs | `bridgeable` and grants opt-in per share; one quota per bridge or grant; grant tokens expire |
| Bot reply loops | bot output never triggers bots |
| Malicious relay | sees only ciphertext; router rotates relays that drop traffic |

## Stack

- Go + go-libp2p for node, router, relay; MLS via an existing library (e.g. OpenMLS over FFI, or a Go port).
- Protobuf for wire messages; SQLite for the local log, history, and MLS state.
- Inference adapters: any OpenAI-compatible HTTP backend (llama.cpp server, vLLM, Ollama, LM Studio).

## Roadmap

| Phase | Deliverable | Exit criteria |
|---|---|---|
| 0 | Two peers across NAT; `/ask` routed to the other's llama.cpp over Noise | streamed completion via relay |
| 1 | Rooms: member log DAG + merge, invites, roles, gossip chat (no MLS yet) | 5 people chat and share in one room |
| 2 | MLS group encryption, kick rotation, rendezvous discovery | kicked member is locked out |
| 3 | Router selection, fair queues, quotas, OpenAI endpoint, fingerprint verification | IDE uses a room model transparently |
| 4 | Anchors, history backfill, TUI polish, multi-room, bridges, receipts | public alpha |
| 5 | Cross-room grants: a share's owner, with room A owner approval, lets room B members use it via B-op tokens bound to a member key, 24h expiry, one shared quota | B member uses A's share without joining A |
