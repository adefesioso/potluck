//! Model sharing: provider, router, heartbeats, and the local OpenAI-compatible endpoint.

use std::{
    collections::BTreeMap,
    fmt::Display,
    fs,
    io::{Read as _, Write as _},
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow};
use axum::{
    Json,
    body::Body,
    extract::{Path, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use futures::{
    AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, StreamExt,
    io::{AsyncRead, BufReader},
};
use libp2p::{PeerId, Stream, StreamProtocol};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::{compat::FuturesAsyncReadCompatExt, io::ReaderStream};

use super::Node;
use crate::room::{Event, Share, Signed, nonce, now};

pub const ASK: StreamProtocol = StreamProtocol::new("/potluck/ask/1");
pub const BEAT_EVERY: Duration = Duration::from_secs(15);
const FRESH: Duration = Duration::from_secs(45);
const MAX_REQUEST: u64 = 1 << 20;

pub struct Beat {
    ts: u64,
    at: Instant,
    models: Vec<String>,
    busy: u32,
}

/// First line of every `/potluck/ask/1` reply; the backend's body follows.
#[derive(Serialize, Deserialize)]
pub struct Head {
    status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn write_head(s: &mut Stream, status: u16, error: Option<String>) -> Result<()> {
    let mut line = serde_json::to_vec(&Head { status, error })?;
    line.push(b'\n');
    Ok(s.write_all(&line).await?)
}

/// Holds one concurrency slot of a share until dropped.
struct Slot {
    node: Arc<Node>,
    model: String,
}

impl Drop for Slot {
    fn drop(&mut self) {
        if let Some(n) = self.node.lock().inflight.get_mut(&self.model) {
            *n -= 1;
        }
    }
}

impl Node {
    /// Floods a signed heartbeat with our shared models and load.
    pub(super) fn beat(self: &Arc<Self>) {
        let event = {
            let st = self.lock();
            Event::Heartbeat {
                room: st.room.id.clone(),
                ts: nonce(),
                models: st.room.local.shares.keys().cloned().collect(),
                busy: st.inflight.values().sum(),
            }
        };
        if let Ok(s) = Signed::new(&self.key, &event) {
            self.cast(self.me, s);
        }
    }

    pub(super) fn on_beat(
        st: &mut super::State,
        by: PeerId,
        ts: u64,
        models: Vec<String>,
        busy: u32,
    ) {
        if st.beats.get(&by).is_none_or(|b| b.ts < ts) {
            st.beats.insert(
                by,
                Beat {
                    ts,
                    at: Instant::now(),
                    models,
                    busy,
                },
            );
        }
    }

    /// Online providers per model, as (peer, nick, busy).
    pub(super) fn online_models(&self) -> BTreeMap<String, Vec<(PeerId, String, u32)>> {
        let st = self.lock();
        let members = st.room.members();
        let mut models: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for (p, b) in &st.beats {
            let Some(nick) = members.get(p) else { continue };
            if b.at.elapsed() < FRESH && st.online.contains(p) {
                for m in &b.models {
                    models
                        .entry(m.clone())
                        .or_default()
                        .push((*p, nick.clone(), b.busy));
                }
            }
        }
        models
    }

    /// Candidates for `model`: pinned first, then least busy; blocked members never.
    fn providers(&self, model: &str) -> Vec<(PeerId, String)> {
        let mut found = self.online_models().remove(model).unwrap_or_default();
        let st = self.lock();
        found.retain(|(p, _, _)| !st.room.local.blocked.contains(p));
        found.sort_by_key(|(p, _, busy)| (!st.room.local.pinned.contains(p), *busy));
        found.into_iter().map(|(p, nick, _)| (p, nick)).collect()
    }

    /// Sends a request to the best provider that accepts it.
    pub(super) async fn route(
        &self,
        body: &Value,
    ) -> Result<(String, Head, impl AsyncRead + Unpin + Send + 'static)> {
        let model = body["model"].as_str().context("request has no model")?;
        let req = serde_json::to_vec(body)?;
        let mut last = anyhow!("no provider online for {model}");
        for (peer, nick) in self.providers(model) {
            match self.try_provider(peer, &req).await {
                Ok((head, _)) if matches!(head.status, 429 | 503) => {
                    last = anyhow!("{nick}: {}", head.error.unwrap_or_default());
                }
                Ok((head, body)) => return Ok((nick, head, body)),
                Err(e) => last = e.context(format!("{nick} unreachable")),
            }
        }
        Err(last)
    }

    async fn try_provider(&self, peer: PeerId, req: &[u8]) -> Result<(Head, BufReader<Stream>)> {
        let mut s = self.control.clone().open_stream(peer, ASK).await?;
        s.write_all(req).await?;
        s.close().await?; // half-close: provider reads to EOF
        let mut body = BufReader::new(s);
        let mut line = String::new();
        (&mut body).take(4096).read_line(&mut line).await?;
        Ok((
            serde_json::from_str(&line).context("bad provider reply")?,
            body,
        ))
    }

    /// Provider side of `/potluck/ask/1`.
    pub(super) async fn provide(self: Arc<Self>, peer: PeerId, mut stream: Stream) {
        let mut body = Vec::new();
        let read = (&mut stream).take(MAX_REQUEST).read_to_end(&mut body).await;
        let mut req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let model = req["model"].as_str().unwrap_or_default().to_owned();
        let checked = match read {
            Ok(_) if req.is_object() => self.take_slot(peer, &model),
            _ => Err((400, "bad request".into())),
        };
        let status = match checked {
            Ok((share, _slot)) => self.forward(&mut stream, share, &mut req, &model).await,
            Err((status, error)) => {
                let _ = write_head(&mut stream, status, Some(error)).await;
                status
            }
        };
        let _ = stream.close().await;
        self.log(peer, &model, status);
    }

    /// Admission for one request: membership, block list, share, rate, concurrency.
    fn take_slot(
        self: &Arc<Self>,
        peer: PeerId,
        model: &str,
    ) -> Result<(Share, Slot), (u16, String)> {
        let st = &mut *self.lock();
        if !st.room.is_member(&peer) || st.room.local.blocked.contains(&peer) {
            return Err((403, "not allowed".into()));
        }
        let share = st.room.local.shares.get(model).cloned();
        let share = share.ok_or_else(|| (404, format!("{model} is not shared here")))?;
        let hits = st.hits.entry(peer).or_default();
        hits.retain(|t| t.elapsed() < Duration::from_secs(60));
        if hits.len() >= share.per_min as usize {
            return Err((429, "rate limit".into()));
        }
        let busy = st.inflight.entry(model.into()).or_default();
        if *busy >= share.max_concurrent {
            return Err((429, "busy".into()));
        }
        *busy += 1;
        hits.push(Instant::now());
        Ok((
            share,
            Slot {
                node: self.clone(),
                model: model.into(),
            },
        ))
    }

    async fn forward(
        &self,
        stream: &mut Stream,
        share: Share,
        req: &mut Value,
        model: &str,
    ) -> u16 {
        if let Some(cap) = share.max_tokens {
            let mut capped = false;
            for k in ["max_tokens", "max_completion_tokens"] {
                if let Some(v) = req[k].as_u64() {
                    req[k] = v.min(cap).into();
                    capped = true;
                }
            }
            if !capped {
                req["max_tokens"] = cap.into();
            }
        }
        let url = format!(
            "{}/v1/chat/completions",
            share.backend.trim_end_matches('/')
        );
        let sent = self
            .http
            .post(url)
            .header("content-type", "application/json")
            .body(req.to_string())
            .send()
            .await;
        let resp = match sent {
            Ok(r) => r,
            Err(e) => {
                let _ = write_head(stream, 502, Some(format!("backend unreachable: {e}"))).await;
                return 502;
            }
        };
        let status = resp.status().as_u16();
        if write_head(stream, status, None).await.is_err() {
            return status;
        }
        let mut chunks = resp.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            if !self.lock().room.local.shares.contains_key(model) {
                let _ = stream
                    .write_all(b"\n\ndata: {\"error\":{\"message\":\"model unshared\"}}\n\n")
                    .await;
                break;
            }
            let Ok(chunk) = chunk else { break };
            if stream.write_all(&chunk).await.is_err() || stream.flush().await.is_err() {
                break;
            }
        }
        status
    }

    fn log(&self, peer: PeerId, model: &str, status: u16) {
        let nick = self
            .lock()
            .room
            .members()
            .get(&peer)
            .cloned()
            .unwrap_or_else(|| peer.to_string());
        let line = format!("{} {nick} {model} {status}\n", now());
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path);
        if let Err(e) = file.and_then(|mut f| f.write_all(line.as_bytes())) {
            eprintln!("* can't write request log: {e}");
        }
    }

    /// `/ask` from the REPL: prints the whole answer as one line when it's done.
    pub(super) fn ask(self: &Arc<Self>, model: String, prompt: String) {
        let node = self.clone();
        tokio::spawn(async move {
            let body = json!({"model": model, "stream": true, "messages": [{"role": "user", "content": prompt}]});
            match node.route(&body).await {
                Ok((nick, Head { status: 200, .. }, body)) => match read_sse(body).await {
                    Ok(text) => println!("<{model}@{nick}> {text}"),
                    Err(e) => println!("* {nick}: {e:#}"),
                },
                Ok((nick, Head { status, error }, _)) => {
                    println!("* {nick}: {status} {}", error.unwrap_or_default())
                }
                Err(e) => println!("* {e:#}"),
            }
        });
    }
}

/// Concatenates the deltas of an OpenAI SSE stream.
async fn read_sse(body: impl AsyncRead + Unpin) -> Result<String> {
    let mut text = String::new();
    let mut lines = BufReader::new(body).lines();
    while let Some(line) = lines.next().await {
        let Some(data) = line?.strip_prefix("data: ").map(str::to_owned) else {
            continue;
        };
        if data == "[DONE]" {
            break;
        }
        let event: Value = serde_json::from_str(&data)?;
        if let Some(msg) = event["error"]["message"].as_str() {
            anyhow::bail!("{msg}");
        }
        text += event["choices"][0]["delta"]["content"]
            .as_str()
            .unwrap_or_default();
    }
    Ok(text)
}

pub fn new_token() -> Result<String> {
    let mut buf = [0; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut buf)?;
    Ok(B64.encode(buf))
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn oai_error(status: StatusCode, msg: impl Display) -> Response {
    (
        status,
        Json(json!({"error": {"message": msg.to_string(), "type": "potluck"}})),
    )
        .into_response()
}

/// Serves the local OpenAI-compatible endpoint until the process exits.
pub async fn api(node: Arc<Node>, addr: SocketAddr) {
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => return println!("* api unavailable on {addr}: {e}"),
    };
    let name = node.lock().room.name();
    println!(
        "* api http://{}/r/{name}/v1",
        listener.local_addr().unwrap()
    );
    let app = axum::Router::new()
        .route("/r/{room}/v1/models", get(models))
        .route("/r/{room}/v1/chat/completions", post(completions))
        .route_layer(middleware::from_fn_with_state(node.clone(), guard))
        .with_state(node);
    if let Err(e) = axum::serve(listener, app).await {
        println!("* api stopped: {e}");
    }
}

/// Local callers only: no browser origin, loopback Host (blocks DNS rebinding), valid token.
async fn guard(
    State(node): State<Arc<Node>>,
    Path(room): Path<String>,
    req: Request,
    next: Next,
) -> Response {
    let h = req.headers();
    if h.contains_key(header::ORIGIN) {
        return oai_error(StatusCode::FORBIDDEN, "browser requests are not allowed");
    }
    let host = h
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let hostname = host.rsplit_once(':').map_or(host, |(name, _)| name);
    if !matches!(hostname, "127.0.0.1" | "localhost" | "[::1]") {
        return oai_error(StatusCode::FORBIDDEN, "non-local Host");
    }
    let auth = h
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let token = auth.strip_prefix("Bearer ").unwrap_or_default();
    let denied = {
        let st = node.lock();
        if room != st.room.name() && room != st.room.id {
            Some(oai_error(StatusCode::NOT_FOUND, "no such room"))
        } else if token.is_empty() || !st.room.local.tokens.values().any(|t| same(t, token)) {
            Some(oai_error(
                StatusCode::UNAUTHORIZED,
                "bad token; create one with /token <app>",
            ))
        } else {
            None
        }
    };
    match denied {
        Some(r) => r,
        None => next.run(req).await,
    }
}

async fn models(State(node): State<Arc<Node>>) -> Json<Value> {
    let data: Vec<_> = node
        .online_models()
        .into_keys()
        .map(|id| json!({"id": id, "object": "model", "owned_by": "potluck"}))
        .collect();
    Json(json!({"object": "list", "data": data}))
}

async fn completions(State(node): State<Arc<Node>>, Json(body): Json<Value>) -> Response {
    let streaming = body["stream"].as_bool().unwrap_or(false);
    let (nick, head, reply) = match node.route(&body).await {
        Ok(r) => r,
        Err(e) => return oai_error(StatusCode::SERVICE_UNAVAILABLE, format!("{e:#}")),
    };
    let status = StatusCode::from_u16(head.status).unwrap_or(StatusCode::BAD_GATEWAY);
    if let Some(error) = head.error {
        return oai_error(status, format!("{nick}: {error}"));
    }
    let content_type = if streaming {
        "text/event-stream"
    } else {
        "application/json"
    };
    let nick: String = nick.chars().filter(char::is_ascii_graphic).collect();
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header("x-potluck-provider", nick)
        .body(Body::from_stream(ReaderStream::new(reply.compat())))
        .unwrap()
}
