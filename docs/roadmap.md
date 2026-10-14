# Roadmap

Status: phase 2 done; public alpha.

## Phases

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
| Share visibility for chosen members | owners want to share with part of a room |
| Local-first preemption, active hours | owners share GPUs they also use daily |
| `/ask -r`: post question and answer to the room | members want to discuss answers together |
