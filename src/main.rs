mod net;

use std::{io::Write as _, path::PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use futures::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, StreamExt, io::BufReader};
use libp2p::{Multiaddr, Stream, StreamProtocol, multiaddr::Protocol, swarm::SwarmEvent};
use serde_json::{Value, json};

const PROTOCOL: StreamProtocol = StreamProtocol::new("/potluck/ask/0");
const MAX_REQUEST: u64 = 1 << 20;

#[derive(Parser)]
#[command(about = "Share local models peer-to-peer")]
struct Cli {
    /// Identity key file [default: ~/.config/potluck/key]
    #[arg(long, global = true)]
    key: Option<PathBuf>,
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
    /// Serve a localhost OpenAI-compatible backend to peers
    Serve {
        /// Relay address, ending in /p2p/<relay-id>
        #[arg(long)]
        relay: Multiaddr,
        #[arg(long, default_value = "http://127.0.0.1:8080")]
        backend: String,
        /// The only model name peers may request
        #[arg(long)]
        model: String,
    },
    /// Ask a provider's model and stream the answer
    Ask {
        /// Provider address printed by `serve`
        #[arg(long)]
        peer: Multiaddr,
        #[arg(long)]
        model: String,
        prompt: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut swarm = net::swarm(net::load_key(cli.key)?)?;
    let me = *swarm.local_peer_id();
    match cli.cmd {
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
        Cmd::Serve {
            relay,
            backend,
            model,
        } => {
            swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;
            swarm.listen_on("/ip4/0.0.0.0/udp/0/quic-v1".parse()?)?;
            swarm.listen_on(relay.with(Protocol::P2pCircuit))?;
            let mut incoming = swarm.behaviour().stream.new_control().accept(PROTOCOL)?;
            let http = reqwest::Client::new();
            let url = format!("{}/v1/chat/completions", backend.trim_end_matches('/'));
            tokio::spawn(async move {
                while let Some((_, stream)) = incoming.next().await {
                    tokio::spawn(serve(stream, http.clone(), url.clone(), model.clone()));
                }
            });
            loop {
                if let SwarmEvent::NewListenAddr { address, .. } = swarm.select_next_some().await
                    && address.iter().any(|p| p == Protocol::P2pCircuit)
                {
                    println!("serving {}", address.with_p2p(me).unwrap_or_else(|a| a));
                }
            }
        }
        Cmd::Ask {
            peer,
            model,
            prompt,
        } => {
            let peer_id = peer
                .iter()
                .filter_map(|p| match p {
                    Protocol::P2p(id) => Some(id),
                    _ => None,
                })
                .last()
                .context("--peer must end in /p2p/<provider-id>")?;
            swarm.dial(peer)?;
            loop {
                match swarm.select_next_some().await {
                    SwarmEvent::ConnectionEstablished { peer_id: p, .. } if p == peer_id => break,
                    SwarmEvent::OutgoingConnectionError {
                        peer_id: Some(p),
                        error,
                        ..
                    } if p == peer_id => {
                        bail!("can't reach provider: {error}")
                    }
                    _ => {}
                }
            }
            let mut control = swarm.behaviour().stream.new_control();
            tokio::spawn(async move {
                loop {
                    swarm.select_next_some().await;
                }
            });
            let body = json!({"model": model, "messages": [{"role": "user", "content": prompt}]});
            ask(control.open_stream(peer_id, PROTOCOL).await?, body).await
        }
    }
}

/// Provider side: forward one request to the backend and pipe its SSE stream back.
async fn serve(mut stream: Stream, http: reqwest::Client, url: String, model: String) {
    if let Err(e) = forward(&mut stream, &http, &url, &model).await {
        let event = json!({"error": {"message": format!("{e:#}")}});
        let _ = stream
            .write_all(format!("data: {event}\n\n").as_bytes())
            .await;
    }
    let _ = stream.close().await;
}

async fn forward(
    stream: &mut Stream,
    http: &reqwest::Client,
    url: &str,
    model: &str,
) -> Result<()> {
    let mut body = Vec::new();
    (&mut *stream)
        .take(MAX_REQUEST)
        .read_to_end(&mut body)
        .await?;
    let mut req: Value = serde_json::from_slice(&body).context("bad request")?;
    if req["model"] != model {
        bail!("model not shared here: {}", req["model"]);
    }
    req["stream"] = true.into();
    let resp = http
        .post(url)
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&req)?)
        .send()
        .await
        .context("backend unreachable")?
        .error_for_status()?;
    let mut chunks = resp.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        stream.write_all(&chunk?).await?;
        stream.flush().await?;
    }
    Ok(())
}

/// Requester side: send the request, print streamed tokens.
async fn ask(mut stream: Stream, body: Value) -> Result<()> {
    stream.write_all(&serde_json::to_vec(&body)?).await?;
    stream.close().await?; // half-close: provider reads to EOF
    let mut lines = BufReader::new(stream).lines();
    let mut out = std::io::stdout();
    while let Some(line) = lines.next().await {
        let Some(data) = line?.strip_prefix("data: ").map(str::to_owned) else {
            continue;
        };
        if data == "[DONE]" {
            break;
        }
        let event: Value = serde_json::from_str(&data)?;
        if let Some(msg) = event["error"]["message"].as_str() {
            bail!("provider: {msg}");
        }
        if let Some(text) = event["choices"][0]["delta"]["content"].as_str() {
            out.write_all(text.as_bytes())?;
            out.flush()?;
        }
    }
    println!();
    Ok(())
}
