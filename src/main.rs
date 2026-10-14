#[macro_use]
mod ui;
mod net;
mod node;
mod room;

use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use futures::StreamExt;
use libp2p::{Multiaddr, multiaddr::Protocol, swarm::SwarmEvent};

#[derive(Parser)]
#[command(about = "Share local models peer-to-peer")]
struct Cli {
    /// State directory holding `key` and `rooms/` [default: ~/.config/potluck]
    #[arg(long, global = true)]
    dir: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Relay circuits for peers behind NAT
    Relay {
        #[arg(long, default_value_t = 4001)]
        port: u16,
    },
    /// Create a room you own
    Create {
        name: String,
        #[arg(long, env = "USER", default_value = "anon")]
        nick: String,
    },
    /// Chat in a room (by name or id prefix)
    Chat {
        room: String,
        #[command(flatten)]
        opts: NodeOpts,
    },
    /// Join a room with a ticket from `/invite`, then chat
    Join {
        ticket: String,
        #[arg(long, env = "USER", default_value = "anon")]
        nick: String,
        #[command(flatten)]
        opts: NodeOpts,
    },
}

#[derive(clap::Args)]
struct NodeOpts {
    /// Relay to listen through when behind NAT
    #[arg(long)]
    relay: Option<Multiaddr>,
    /// Local OpenAI-compatible endpoint
    #[arg(long, default_value = "127.0.0.1:7777")]
    api: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let dir = cli.dir.unwrap_or_else(|| {
        PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config/potluck")
    });
    let rooms = dir.join("rooms");
    fs::create_dir_all(&rooms)?;
    let key = net::load_key(&dir.join("key"))?;
    let mut swarm = net::swarm(key.clone())?;
    let me = *swarm.local_peer_id();
    match cli.cmd {
        Cmd::Create { name, nick } => {
            let room = room::Room::create(&key, &name, &nick)?;
            fs::write(
                rooms.join(format!("{}.json", room.id)),
                serde_json::to_vec(&room)?,
            )?;
            println!("room {}", room.id);
            Ok(())
        }
        Cmd::Chat { room, opts } => {
            let (path, room) = find_room(&rooms, &room)?;
            net::listen(&mut swarm, opts.relay)?;
            let wire = node::spawn_swarm(swarm);
            let done = node::run(key, path, room, wire, opts.api, ui::mode()).await;
            ui::restore();
            done
        }
        Cmd::Join { ticket, nick, opts } => {
            net::listen(&mut swarm, opts.relay)?;
            let ticket = room::Ticket::decode(&ticket)?;
            let done = node::join(key, rooms, ticket, nick, swarm, opts.api, ui::mode()).await;
            ui::restore();
            done
        }
        Cmd::Relay { port } => {
            swarm.listen_on(format!("/ip4/0.0.0.0/tcp/{port}").parse()?)?;
            swarm.listen_on(format!("/ip4/0.0.0.0/udp/{port}/quic-v1").parse()?)?;
            loop {
                if let SwarmEvent::NewListenAddr { address, .. } = swarm.select_next_some().await {
                    // ponytail: trusts every listen address as reachable; add --external when behind NAT/cloud LB.
                    swarm.add_external_address(address.clone());
                    println!("listening {}", address.with(Protocol::P2p(me)));
                }
            }
        }
    }
}

/// Finds exactly one saved room whose name or id prefix matches.
fn find_room(rooms: &Path, query: &str) -> Result<(PathBuf, room::Room)> {
    let mut found = Vec::new();
    for entry in fs::read_dir(rooms)? {
        let path = entry?.path();
        let room: room::Room = serde_json::from_slice(&fs::read(&path)?)?;
        if room.name() == query || room.id.starts_with(query) {
            found.push((path, room));
        }
    }
    match found.len() {
        1 => Ok(found.pop().unwrap()),
        0 => bail!("no room matches {query}"),
        _ => bail!("{query} matches several rooms; use an id prefix"),
    }
}
