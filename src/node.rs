//! Room node: keeps connections to members, merges events, floods chat, runs the REPL.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use futures::{AsyncReadExt, AsyncWriteExt, StreamExt};
use libp2p::{
    Multiaddr, PeerId, Stream, StreamProtocol, Swarm,
    identity::Keypair,
    multiaddr::Protocol,
    swarm::{
        SwarmEvent,
        dial_opts::{DialOpts, PeerCondition},
    },
};
use libp2p_stream::Control;
use serde::{Deserialize, Serialize};
use tokio::{io::AsyncBufReadExt, sync::mpsc};

use crate::{
    net::Behaviour,
    room::{Event, Room, Signed, Ticket, nonce, now},
};

const PROTOCOL: StreamProtocol = StreamProtocol::new("/potluck/room/1");
const MAX_MSG: u64 = 4 << 20;

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Msg {
    Join {
        join: Signed,
    },
    Sync {
        events: Vec<Signed>,
        peers: Vec<Multiaddr>,
    },
    Chat {
        chat: Signed,
    },
    Denied {
        reason: String,
    },
}

enum Net {
    Up(PeerId),
    Down(PeerId),
    Addr(Multiaddr),
}

enum Cmd {
    Dial(PeerId, Vec<Multiaddr>),
    Drop(PeerId),
}

/// The swarm runs in its own task; the node talks to it through these.
pub struct Wire {
    control: Control,
    cmds: mpsc::UnboundedSender<Cmd>,
    events: mpsc::UnboundedReceiver<Net>,
}

pub fn spawn_swarm(mut swarm: Swarm<Behaviour>) -> Wire {
    let control = swarm.behaviour().stream.new_control();
    let me = *swarm.local_peer_id();
    let (cmds, mut cmd_rx) = mpsc::unbounded_channel();
    let (ev_tx, events) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                ev = swarm.select_next_some() => {
                    let net = match ev {
                        SwarmEvent::ConnectionEstablished { peer_id, num_established, .. }
                            if num_established.get() == 1 => Net::Up(peer_id),
                        SwarmEvent::ConnectionClosed { peer_id, num_established: 0, .. } => Net::Down(peer_id),
                        SwarmEvent::NewListenAddr { address, .. } => {
                            Net::Addr(address.with_p2p(me).unwrap_or_else(|a| a))
                        }
                        _ => continue,
                    };
                    if ev_tx.send(net).is_err() {
                        return;
                    }
                }
                Some(cmd) = cmd_rx.recv() => match cmd {
                    Cmd::Dial(peer, addrs) => {
                        let opts = DialOpts::peer_id(peer)
                            .addresses(addrs)
                            .condition(PeerCondition::DisconnectedAndNotDialing)
                            .build();
                        let _ = swarm.dial(opts);
                    }
                    Cmd::Drop(peer) => {
                        let _ = swarm.disconnect_peer_id(peer);
                    }
                },
            }
        }
    });
    Wire {
        control,
        cmds,
        events,
    }
}

async fn write(s: &mut Stream, msg: &Msg) -> Result<()> {
    s.write_all(&serde_json::to_vec(msg)?).await?;
    Ok(s.close().await?) // half-close: the reader reads to EOF
}

async fn read(s: &mut Stream) -> Result<Option<Msg>> {
    let mut buf = Vec::new();
    (&mut *s).take(MAX_MSG).read_to_end(&mut buf).await?;
    if buf.is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_slice(&buf)?))
}

async fn send(mut control: Control, peer: PeerId, msg: &Msg) -> Result<Option<Msg>> {
    let mut s = control.open_stream(peer, PROTOCOL).await?;
    write(&mut s, msg).await?;
    read(&mut s).await
}

fn by_peer(addrs: impl IntoIterator<Item = Multiaddr>) -> HashMap<PeerId, Vec<Multiaddr>> {
    let mut map: HashMap<_, Vec<_>> = HashMap::new();
    for a in addrs {
        if let Some(Protocol::P2p(p)) = a.iter().last() {
            map.entry(p).or_default().push(a);
        }
    }
    map
}

struct State {
    room: Room,
    online: HashSet<PeerId>,
    /// Our own listen addresses.
    addrs: BTreeSet<Multiaddr>,
    /// Chat ids already delivered, for flood dedup.
    // ponytail: grows for the life of the process; prune by age if sessions run for weeks.
    seen: HashSet<String>,
}

struct Node {
    key: Keypair,
    me: PeerId,
    path: PathBuf,
    control: Control,
    cmds: mpsc::UnboundedSender<Cmd>,
    state: Mutex<State>,
}

impl Node {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    fn save(&self, st: &State) {
        if let Err(e) = fs::write(&self.path, serde_json::to_vec(&st.room).unwrap()) {
            eprintln!("* can't save room: {e}");
        }
    }

    fn sync_msg(&self, st: &State) -> Msg {
        let members = st.room.members();
        let peers = st.room.peers.iter().chain(&st.addrs);
        Msg::Sync {
            events: st.room.events.values().cloned().collect(),
            peers: by_peer(peers.cloned())
                .into_iter()
                .filter(|(p, _)| members.contains_key(p))
                .flat_map(|(_, a)| a)
                .collect(),
        }
    }

    fn online_members(&self, st: &State) -> Vec<PeerId> {
        st.online
            .iter()
            .filter(|p| st.room.is_member(p))
            .copied()
            .collect()
    }

    fn dial_members(&self, st: &State) {
        for (peer, addrs) in by_peer(st.room.peers.iter().cloned()) {
            if peer != self.me && !st.online.contains(&peer) && st.room.is_member(&peer) {
                let _ = self.cmds.send(Cmd::Dial(peer, addrs));
            }
        }
    }

    /// Merges events and addresses; announces membership changes and passes new events on.
    fn merge(self: &Arc<Self>, from: Option<PeerId>, events: Vec<Signed>, peers: Vec<Multiaddr>) {
        let mut st = self.lock();
        let before = st.room.members();
        let mut changed = false;
        for e in events {
            changed |= st.room.add(e).unwrap_or(false);
        }
        let mut new_peers = false;
        for a in peers {
            if matches!(a.iter().last(), Some(Protocol::P2p(p)) if p != self.me) {
                new_peers |= st.room.peers.insert(a);
            }
        }
        let after = st.room.members();
        for (p, nick) in &after {
            if !before.contains_key(p) {
                println!("* {nick} joined");
            }
        }
        for (p, nick) in &before {
            if !after.contains_key(p) {
                println!("* {nick} removed");
                let _ = self.cmds.send(Cmd::Drop(*p));
            }
        }
        if changed || new_peers {
            self.save(&st);
            self.dial_members(&st);
        }
        let targets = if changed {
            self.online_members(&st)
        } else {
            vec![]
        };
        drop(st);
        for p in targets.into_iter().filter(|p| Some(*p) != from) {
            self.sync_with(p);
        }
    }

    fn sync_with(self: &Arc<Self>, peer: PeerId) {
        let node = self.clone();
        let msg = self.sync_msg(&self.lock());
        tokio::spawn(async move {
            match send(node.control.clone(), peer, &msg).await {
                Ok(Some(Msg::Sync { events, peers })) => node.merge(Some(peer), events, peers),
                Ok(Some(Msg::Denied { reason })) => println!("* {peer} refused sync: {reason}"),
                _ => {}
            }
        });
    }

    /// Delivers a chat message once and forwards it to every other online member.
    fn chat(self: &Arc<Self>, from: PeerId, chat: Signed) {
        let st = &mut *self.lock();
        let Ok(Event::Chat { room, text, .. }) = chat.open() else {
            return;
        };
        let Some(nick) = st.room.members().get(&chat.by).cloned() else {
            return;
        };
        if room != st.room.id || !st.seen.insert(chat.id()) {
            return;
        }
        if chat.by != self.me {
            println!("<{nick}> {text}");
        }
        for p in self.online_members(st) {
            if p != from && p != chat.by {
                let (control, msg) = (self.control.clone(), Msg::Chat { chat: chat.clone() });
                tokio::spawn(async move { send(control, p, &msg).await });
            }
        }
    }

    fn admit(self: &Arc<Self>, peer: PeerId, join: Signed) -> Result<Msg> {
        ensure!(join.by == peer, "join must be signed by the joiner");
        self.lock().room.admit(&join)?;
        self.merge(Some(peer), vec![join], vec![]);
        Ok(self.sync_msg(&self.lock()))
    }

    async fn handle(self: Arc<Self>, peer: PeerId, mut stream: Stream) -> Result<()> {
        let member = self.lock().room.is_member(&peer);
        let denied = |reason: &str| {
            Some(Msg::Denied {
                reason: reason.into(),
            })
        };
        let reply = match read(&mut stream).await? {
            Some(Msg::Join { join }) => {
                Some(self.admit(peer, join).unwrap_or_else(|e| Msg::Denied {
                    reason: format!("{e:#}"),
                }))
            }
            Some(Msg::Sync { .. } | Msg::Chat { .. }) if !member => denied("not a member"),
            Some(Msg::Sync { events, peers }) => {
                self.merge(Some(peer), events, peers);
                Some(self.sync_msg(&self.lock()))
            }
            Some(Msg::Chat { chat }) => {
                self.chat(peer, chat);
                None
            }
            _ => None,
        };
        match reply {
            Some(r) => write(&mut stream, &r).await,
            None => Ok(stream.close().await?),
        }
    }

    /// Runs one REPL line; returns true to quit.
    fn command(self: &Arc<Self>, line: &str) -> Result<bool> {
        let mut words = line.split_whitespace();
        let (room_id, owner) = {
            let st = self.lock();
            (st.room.id.clone(), st.room.owner() == self.me)
        };
        let owner_only = || -> Result<()> {
            ensure!(owner, "only the owner can do that");
            Ok(())
        };
        match words.next() {
            Some("/quit") => return Ok(true),
            Some("/who") => {
                let st = self.lock();
                for (p, nick) in st.room.members() {
                    let role = if p == st.room.owner() {
                        "owner"
                    } else {
                        "member"
                    };
                    let here = if p == self.me {
                        "you"
                    } else if st.online.contains(&p) {
                        "online"
                    } else {
                        "offline"
                    };
                    println!("* {nick} {p} {role} {here}");
                }
            }
            Some("/invite") => {
                owner_only()?;
                let uses = words.next().map(str::parse).transpose()?.unwrap_or(1);
                let hours: u64 = words.next().map(str::parse).transpose()?.unwrap_or(24);
                let invitee = words.next().map(str::parse).transpose()?;
                let invite = Event::Invite {
                    room: room_id,
                    invitee,
                    uses,
                    expires: now() + hours * 3600,
                    nonce: nonce(),
                };
                let invite = Signed::new(&self.key, &invite)?;
                let Msg::Sync { mut peers, .. } = self.sync_msg(&self.lock()) else {
                    unreachable!()
                };
                peers.sort_by_key(|a| a.iter().last() != Some(Protocol::P2p(self.me))); // ours first
                println!(
                    "* invite {} {}",
                    invite.id(),
                    Ticket { invite, peers }.encode()
                );
            }
            Some("/revoke") => {
                owner_only()?;
                let invite = words.next().context("usage: /revoke <invite-id>")?.into();
                let revoke = Signed::new(
                    &self.key,
                    &Event::Revoke {
                        room: room_id,
                        invite,
                    },
                )?;
                self.merge(None, vec![revoke], vec![]);
            }
            Some("/remove") => {
                owner_only()?;
                let who = words.next().context("usage: /remove <nick|peer-id>")?;
                let removes = {
                    let st = self.lock();
                    let members = st.room.members();
                    let peer = members
                        .iter()
                        .find(|(p, n)| *n == who || p.to_string() == who);
                    let (peer, _) = peer.context("no such member")?;
                    ensure!(*peer != self.me, "can't remove yourself");
                    st.room.remove(&self.key, peer)?
                };
                self.merge(None, removes, vec![]);
            }
            Some(w) if w.starts_with('/') => bail!("unknown command {w}"),
            Some(_) => {
                let chat = Signed::new(
                    &self.key,
                    &Event::Chat {
                        room: room_id,
                        text: line.into(),
                        nonce: nonce(),
                    },
                )?;
                self.chat(self.me, chat);
            }
            None => {}
        }
        Ok(false)
    }
}

/// Runs a room node until stdin closes or `/quit`.
pub async fn run(
    key: Keypair,
    path: PathBuf,
    room: Room,
    wire: Wire,
    online: HashSet<PeerId>,
    addrs: BTreeSet<Multiaddr>,
) -> Result<()> {
    let Wire {
        mut control,
        cmds,
        mut events,
    } = wire;
    let me = key.public().to_peer_id();
    let mut incoming = control.accept(PROTOCOL)?;
    println!(
        "* in {} as {}",
        room.name(),
        room.members()
            .get(&me)
            .context("not a member of this room")?
    );
    let node = Arc::new(Node {
        key,
        me,
        path,
        control,
        cmds,
        state: Mutex::new(State {
            room,
            online,
            addrs,
            seen: HashSet::new(),
        }),
    });
    let targets = {
        let st = node.lock();
        node.save(&st);
        node.dial_members(&st);
        node.online_members(&st)
    };
    for p in targets {
        node.sync_with(p);
    }
    let n = node.clone();
    tokio::spawn(async move {
        while let Some((peer, stream)) = incoming.next().await {
            tokio::spawn(n.clone().handle(peer, stream));
        }
    });
    let mut stdin = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    loop {
        tokio::select! {
            ev = events.recv() => match ev.context("network stopped")? {
                Net::Up(p) => {
                    let member = { let mut st = node.lock(); st.online.insert(p); st.room.is_member(&p) };
                    if member {
                        node.sync_with(p);
                    }
                }
                Net::Down(p) => {
                    let st = &mut *node.lock();
                    st.online.remove(&p);
                    node.dial_members(st); // ponytail: one redial per drop; add backoff retries if links flap
                }
                Net::Addr(a) => {
                    println!("* listening {a}");
                    node.lock().addrs.insert(a);
                }
            },
            line = stdin.next_line() => match line? {
                None => return Ok(()),
                Some(line) => match node.command(line.trim()) {
                    Ok(true) => return Ok(()),
                    Ok(false) => {}
                    Err(e) => println!("* {e:#}"),
                },
            },
        }
    }
}

/// Redeems a ticket with any reachable member, then runs the room.
pub async fn join(
    key: Keypair,
    rooms: PathBuf,
    ticket: Ticket,
    nick: String,
    swarm: Swarm<Behaviour>,
) -> Result<()> {
    let Event::Invite { room: room_id, .. } = ticket.invite.open()? else {
        bail!("ticket holds no invite")
    };
    let me = key.public().to_peer_id();
    let join = Signed::new(
        &key,
        &Event::Join {
            room: room_id.clone(),
            nick,
            invite: ticket.invite,
        },
    )?;
    let mut wire = spawn_swarm(swarm);
    let bootstrap = by_peer(ticket.peers);
    for (p, addrs) in &bootstrap {
        let _ = wire.cmds.send(Cmd::Dial(*p, addrs.clone()));
    }
    let (mut online, mut addrs) = (HashSet::new(), BTreeSet::new());
    let room = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            match wire.events.recv().await.context("network stopped")? {
                Net::Up(p) => {
                    online.insert(p);
                    if !bootstrap.contains_key(&p) {
                        continue;
                    }
                    match send(wire.control.clone(), p, &Msg::Join { join: join.clone() }).await {
                        Ok(Some(Msg::Sync { events, peers })) => {
                            let genesis = events
                                .iter()
                                .find(|e| e.id() == room_id)
                                .context("no genesis in reply")?;
                            let mut room = Room::from_genesis(genesis.clone())?;
                            for e in events {
                                room.add(e).ok();
                            }
                            room.peers.extend(
                                peers
                                    .into_iter()
                                    .filter(|a| a.iter().last() != Some(Protocol::P2p(me))),
                            );
                            ensure!(room.is_member(&me), "member didn't admit us");
                            return Ok(room);
                        }
                        Ok(Some(Msg::Denied { reason })) => bail!("join refused: {reason}"),
                        _ => println!("* join via {p} failed, waiting for another member"),
                    }
                }
                Net::Down(p) => {
                    online.remove(&p);
                }
                Net::Addr(a) => {
                    addrs.insert(a);
                }
            }
        }
    })
    .await
    .context("no member reachable from the ticket")??;
    let path = rooms.join(format!("{}.json", room.id));
    run(key, path, room, wire, online, addrs).await
}
