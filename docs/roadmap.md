# Roadmap

Phase 2 is done, and Potluck is in public alpha.

## Phases

| Phase | Deliverable | Exit criteria |
|---|---|---|
| 0 | Two peers across NAT, with `/ask` routed to the other's llama.cpp over Noise | A streamed completion through a relay |
| 1 | Rooms: invites, joins, removal, signed chat, address exchange | Five people chat, and a removed member is locked out |
| 2 | Shares, heartbeats, selection, limits, the OpenAI endpoint, the TUI | An IDE uses a room model; public alpha |

## Known limits

These are shortcuts taken on purpose. Each is marked in the code and gets fixed when it starts to hurt.

- A node does not renew its relay reservation if the connection to the relay drops. Restart the node.
- A dropped connection to a member is redialed once, with no backoff or retries.
- `potluck relay` treats every non-loopback address it listens on as reachable. It has no option to state an external address, which a relay behind NAT or a cloud load balancer would need.

Limits that bear on security are listed in [security.md](security.md#known-gaps).

## Later

Each of these ships when its trigger shows up in real use.

| Feature | Trigger |
|---|---|
| Members as relays | Rooms form without a host to run `potluck relay` |
| Ops and co-owners, which need causal event ordering | One owner can't keep up with invites and removals |
| MLS group encryption | Rooms grow past about 50 members |
| Model verification | Rooms form among people who don't know each other |
| Always-on anchors, history backfill, sender-key encryption at rest | Members want history from while they were offline |
| DHT rendezvous | Members' addresses change faster than they sync |
| Cross-room sharing | Providers ask to serve several rooms from a single share |
| Usage reciprocity | Providers report free-riding |
| Bots and sub-channels | Rooms get busy enough to need them |
| Share visibility for chosen members | Providers want to share with only part of a room |
| Local-first preemption and active hours | Providers share GPUs they also use daily |
| `/ask -r`, which posts the question and answer to the room | Members want to discuss answers together |
