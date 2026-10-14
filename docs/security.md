# Security

Concepts (room, member set, invite, remove): [design.md](design.md#concepts).

## Properties

- **Transport:** every member pair talks over libp2p Noise sessions bound to member keys; relays see only ciphertext.
- **Chat:** each message is signed by its sender and travels only over Noise streams between current members; nodes refuse room streams from non-members, so a removed member receives nothing new.
- **Discovery:** tickets carry bootstrap addresses; members exchange known addresses (relay circuits included) on every sync and redial them. Nothing about a room is published.
- **Provider sandbox:** the node talks only to the localhost inference server, opens no inbound ports, forwards only chat/completion calls for shared models, and never proxies admin APIs (pull, delete, load).
- **Disclosure:** a provider sees the plaintext prompts it serves; the client shows who served each answer.

## Threat model

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
