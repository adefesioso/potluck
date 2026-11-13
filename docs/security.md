# Security

This page uses the terms room, member set, invite and remove as defined in [design.md](design.md#concepts).

## Properties

**Transport.** Every pair of members talks over libp2p Noise sessions bound to their keys. A relay between them sees only ciphertext.

**Chat.** Each message is signed by its sender and travels only over Noise streams between current members. From a non-member, a node accepts nothing but a `join`, reads at most 64 KiB, and waits at most 30 seconds. A removed member therefore receives nothing new.

**Discovery.** A ticket carries addresses of current members to dial. After that, members exchange the addresses they know, relay circuits included, on every sync, and redial them when a connection drops. Nothing about a room is published.

**Provider sandbox.** A provider's node forwards requests only to the backend URL given to `/share`, which is normally on localhost, and only as chat-completion calls for models that are shared. It never proxies a backend's admin APIs such as pull, delete or load. The node listens on libp2p TCP and QUIC ports and on the local API port, and nothing else.

**Disclosure.** A provider sees the plaintext of the prompts it serves. The requester always sees who served each answer, in the TUI and in the `x-potluck-provider` header.

## Threat model

| Threat | Mitigation |
|---|---|
| An outsider reads chat or prompts | Each pair of members has its own Noise session, room streams run only between members, and nothing is published. |
| A removed member keeps access | Members disconnect the removed key and refuse its streams. It can only return on an invite that names it. |
| An invite leaks | Invites expire, are limited in uses, and can be bound to one key. The owner can revoke them. |
| A member abuses someone's GPU | The provider sets limits, keeps a request log, and can block the member. The owner can remove them. |
| A provider snoops on prompts | This is disclosed up front. The requester sees who served each answer and can pin or block providers. |
| A provider misreports which model it runs | Rooms rely on trust between members. The requester can pin the members they trust. |
| A requester reaches a backend's admin APIs | The provider forwards only chat-completion calls for shared models. |
| A local process or website uses the endpoint | The endpoint binds to 127.0.0.1, requires a per-app token, and checks `Origin` and `Host`, which blocks CSRF and DNS rebinding. |
| A relay is malicious | It sees only ciphertext. If it drops traffic, restart with a different `--relay`. |

## Known gaps

- Every node also runs libp2p's relay service, so any peer that can reach it may relay through it within libp2p's rate limits. Incoming connections are capped at 256.
- Chat messages are deduplicated for 10 minutes. A member can replay an older signed message after that; a non-member cannot.
- Requests to a model you share yourself go straight to your backend and skip the share's limits.
