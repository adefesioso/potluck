use std::{
    fs,
    io::{ErrorKind, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    time::Duration,
};

use anyhow::Result;
use libp2p::{
    Multiaddr, Swarm, dcutr, identify, identity::Keypair, multiaddr::Protocol, noise, ping, relay,
    swarm::NetworkBehaviour, tcp, yamux,
};

#[derive(NetworkBehaviour)]
pub struct Behaviour {
    // ponytail: every node is an open relay with libp2p's default rate limits; restrict to room members if relays get abused.
    relay: relay::Behaviour,
    relay_client: relay::client::Behaviour,
    dcutr: dcutr::Behaviour,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
    pub stream: libp2p_stream::Behaviour,
}

/// Loads the identity key, creating it (mode 0600) on first run.
pub fn load_key(path: &Path) -> Result<Keypair> {
    match fs::read(path) {
        Ok(bytes) => return Ok(Keypair::from_protobuf_encoding(&bytes)?),
        Err(e) if e.kind() != ErrorKind::NotFound => return Err(e.into()),
        Err(_) => {}
    }
    let key = Keypair::generate_ed25519();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?
        .write_all(&key.to_protobuf_encoding()?)?;
    Ok(key)
}

pub fn swarm(key: Keypair) -> Result<Swarm<Behaviour>> {
    Ok(libp2p::SwarmBuilder::with_existing_identity(key)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_quic()
        .with_relay_client(noise::Config::new, yamux::Config::default)?
        .with_behaviour(|key, relay_client| {
            let id = key.public().to_peer_id();
            Behaviour {
                relay: relay::Behaviour::new(
                    id,
                    relay::Config {
                        // Completions outlive the 2 min / 128 KiB defaults.
                        max_circuit_duration: Duration::from_secs(3600),
                        max_circuit_bytes: 0,
                        ..Default::default()
                    },
                ),
                relay_client,
                dcutr: dcutr::Behaviour::new(id),
                identify: identify::Behaviour::new(identify::Config::new(
                    "/potluck/0".into(),
                    key.public(),
                )),
                ping: ping::Behaviour::default(),
                stream: libp2p_stream::Behaviour::new(),
            }
        })?
        .with_swarm_config(|c| // Room members stay connected without traffic.
            c.with_idle_connection_timeout(Duration::from_secs(3600)))
        .build())
}

/// Listens on all interfaces, plus a relay circuit for peers behind NAT.
pub fn listen(swarm: &mut Swarm<Behaviour>, relay: Option<Multiaddr>) -> Result<()> {
    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;
    swarm.listen_on("/ip4/0.0.0.0/udp/0/quic-v1".parse()?)?;
    if let Some(relay) = relay {
        swarm.listen_on(relay.with(Protocol::P2pCircuit))?;
    }
    Ok(())
}
