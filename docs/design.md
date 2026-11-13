# Design

Potluck is IRC for AI. People gather in rooms where they chat and share the models they run locally. The software finds the other members, gets through NAT, encrypts everything, and sends each request to a member who can serve it.

## Principles

- **Rooms are the trust boundary.** Members trust each other, and only members see the room's chat, models or traffic.
- **No server is required.** Nodes talk peer to peer. A `potluck relay` on a reachable host helps the ones behind NAT, and it only ever sees ciphertext.
- **Providers stay in control.** Whoever shares a GPU sets its limits and can stop at any time.
- **Rooms are small.** Every mechanism is sized for 2 to 30 people, which is what lets them stay simple.

## Concepts

**Identity.** A node's identity is an ed25519 keypair, which is also its libp2p peer ID. The nickname is only for display.

**Room.** A room starts with a genesis event carrying its name and the owner's nick, signed by the owner. The room's ID is the hash of that event, so the ID alone fixes who the owner is. A room has a member set, chat, and shares.

**Member set.** Membership is a grow-only set of signed events. Members merge each other's sets by union, and the order in which events arrive does not matter. The members are the owner, plus everyone whose join carries an invite signed by the owner, minus every join named by a remove.

**Invite.** An invite is signed by the owner and states the room, how many times it can be used, when it expires, and optionally the one key allowed to use it. It is handed over as a ticket string that also carries addresses of current members to dial. An invite is not published to the room; it travels inside the joins that use it.

**Join.** A join is signed by the invitee and carries the invite and a nick. The nick must be 1 to 32 letters, digits, `-`, `_` or `.`, and must not be in use by a current member, ignoring case. The member who admits the join checks that the invite has not expired, been used up, or been revoked. A key that has joined before needs an invite that names it, so a removed member cannot return on a general invite. Two members admitting at the same moment can let in more people than the invite allows, and the owner removes the extras.

**Revoke.** A revoke is signed by the owner and names an invite, which then admits no one else.

**Remove.** A remove is signed by the owner and names a join. Members drop the removed key's connections and refuse its streams.

**Share.** A share is a provider's local setting and never leaves their node: a model, the backend URL that serves it, a concurrency limit, a per-member rate limit, and an optional token cap. The model is whatever name the backend uses, such as `llama3.1:70b-q4_K_M`.

**Heartbeat.** A provider signs a heartbeat listing its shared models and how many requests it is running. Heartbeats spread through the room like chat, every 15 seconds while the node shares anything and at once when a model is shared or unshared. The newest one wins, and a provider that has been silent for 45 seconds counts as offline.

One hard limit follows from this design: only the owner key can invite and remove. If it is lost, membership freezes and the members have to start a new room.

## Flow

```
owner /invite ──► ticket (invite.txt) ──► bob dials bootstrap peers, signs join, members accept
bob: /ask llama3.1:70b "explain this diff"
  bob's router ──► picks carol (online, shares the model, least busy)
  bob ══Noise══ [relay if NAT] ══ carol's node ──► localhost llama.cpp
  tokens stream back to bob only
```

## Router

**Connectivity.** Nodes use rust-libp2p over QUIC and TCP. Peers behind NAT listen through circuit relay v2 on a `potluck relay`, and DCUtR hole punching upgrades those circuits to direct paths where it can.

**Membership.** Members exchange their event sets and known addresses whenever they connect and whenever something changes. Each side takes the union and recomputes who is in the room.

**Chat.** A message goes to every connected member, and each member forwards messages it has not seen to its other connected members. Duplicates are recognised by the message's hash. There is no history, so members see only what is sent while they are online.

**Resolution and selection.** A model name resolves to the connected members whose latest heartbeat lists it. Pinned members are tried first, then the least busy according to their heartbeats. If a provider is busy, refuses, or cannot be reached, the router moves on to the next. A model you share yourself is answered by your own backend first.

## Provider controls

A provider sets three limits on each share: how many requests run at once, how many requests each member may make per minute, and the most tokens a single request may ask for.

Every request a provider serves is written to a local log with the time, member, model and status. Together with a per-member `/block`, this lets providers see and decide who uses their GPU.

`/unshare` takes effect immediately. New requests are refused and streams in flight end with an error event.
