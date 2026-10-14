//! Phase 2 exit check: an OpenAI client on alice's machine uses a model bob shares.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

struct Peer {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
}

impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

impl Peer {
    fn spawn(dir: &Path, args: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_potluck"))
            .arg("--dir")
            .arg(dir)
            .args(args)
            .args(["--api", "127.0.0.1:0"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        thread::spawn(move || {
            for l in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = tx.send(l);
            }
        });
        let stdin = child.stdin.take().unwrap();
        Self {
            child,
            stdin,
            lines,
        }
    }

    fn say(&mut self, line: &str) {
        writeln!(self.stdin, "{line}").unwrap();
    }

    /// Next line containing `pat`, skipping others.
    fn expect(&mut self, pat: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        while let Ok(l) = self
            .lines
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            if l.contains(pat) {
                return l;
            }
        }
        panic!("timed out waiting for {pat:?}");
    }
}

/// OpenAI-style backend: streams "Hel","lo" or returns one JSON body; reports each request body.
fn fake_backend() -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for conn in listener.incoming() {
            let mut conn = BufReader::new(conn.unwrap());
            let mut len = 0;
            loop {
                let mut line = String::new();
                conn.read_line(&mut line).unwrap();
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
                if line == "\r\n" {
                    break;
                }
            }
            let mut body = vec![0; len];
            conn.read_exact(&mut body).unwrap();
            let body = String::from_utf8(body).unwrap();
            let reply = if body.contains("\"stream\":true") {
                let mut sse = String::new();
                for t in ["Hel", "lo"] {
                    sse += &format!(
                        "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{t}\"}}}}]}}\n\n"
                    );
                }
                format!("content-type: text/event-stream\r\n\r\n{sse}data: [DONE]\n\n")
            } else {
                "content-type: application/json\r\n\r\n{\"choices\":[{\"message\":{\"content\":\"Hello\"}}]}".into()
            };
            let mut conn = conn.into_inner();
            write!(conn, "HTTP/1.1 200 OK\r\nconnection: close\r\n{reply}").unwrap();
            tx.send(body).unwrap();
        }
    });
    (addr, rx)
}

/// HTTP/1.0 so the reply is close-delimited, not chunked. Returns (status, head, body).
fn http(port: &str, path: &str, headers: &[&str], body: Option<&str>) -> (u16, String, String) {
    let mut s = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
    let method = if body.is_some() { "POST" } else { "GET" };
    let mut req = format!("{method} {path} HTTP/1.0\r\nhost: 127.0.0.1:{port}\r\n");
    for h in headers {
        req += &format!("{h}\r\n");
    }
    if let Some(b) = body {
        req += &format!(
            "content-type: application/json\r\ncontent-length: {}\r\n\r\n{b}",
            b.len()
        );
    } else {
        req += "\r\n";
    }
    s.write_all(req.as_bytes()).unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    let (head, body) = resp.split_once("\r\n\r\n").unwrap();
    (
        head[9..12].parse().unwrap(),
        head.to_ascii_lowercase(),
        body.into(),
    )
}

#[test]
fn ide_uses_a_room_model() {
    let root = std::env::temp_dir().join(format!("potluck-p2-{}", std::process::id()));
    let (backend, seen) = fake_backend();

    let created = Command::new(env!("CARGO_BIN_EXE_potluck"))
        .arg("--dir")
        .arg(root.join("alice"))
        .args(["create", "friends", "--nick", "alice"])
        .output()
        .unwrap();
    assert!(created.status.success());
    let mut alice = Peer::spawn(&root.join("alice"), &["chat", "friends"]);
    let api = alice.expect("* api http://");
    let port = api
        .split(':')
        .nth(2)
        .unwrap()
        .split('/')
        .next()
        .unwrap()
        .to_owned();
    alice.expect("* listening /ip4/127.0.0.1/tcp/");
    alice.say("/invite");
    let ticket = alice
        .expect("* invite ")
        .split_whitespace()
        .nth(3)
        .unwrap()
        .to_owned();
    let mut bob = Peer::spawn(&root.join("bob"), &["join", &ticket, "--nick", "bob"]);
    alice.expect("* bob joined");

    bob.say(&format!("/share m http://{backend} 1 30 5"));
    bob.expect("* sharing m");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        alice.say("/models");
        if alice.expect("* ").starts_with("* m: bob") {
            break;
        }
        assert!(Instant::now() < deadline, "bob's share never showed up");
        thread::sleep(Duration::from_millis(200));
    }

    alice.say("/token ide");
    let token = alice
        .expect("* token ide ")
        .split_whitespace()
        .nth(3)
        .unwrap()
        .to_owned();
    let auth = format!("authorization: Bearer {token}");
    let base = "/r/friends/v1";

    // Endpoint guard.
    assert_eq!(http(&port, &format!("{base}/models"), &[], None).0, 401);
    assert_eq!(
        http(
            &port,
            &format!("{base}/models"),
            &["authorization: Bearer nope"],
            None
        )
        .0,
        401
    );
    assert_eq!(
        http(
            &port,
            &format!("{base}/models"),
            &[&auth, "origin: http://evil.example"],
            None
        )
        .0,
        403
    );
    let (status, _, body) = http(&port, &format!("{base}/models"), &[&auth], None);
    assert_eq!(status, 200);
    assert!(body.contains("\"id\":\"m\""), "{body}");

    // Streaming and non-streaming completions, routed to bob, max_tokens clamped to 5.
    let completions = format!("{base}/chat/completions");
    let req = |stream: bool| {
        format!(
            r#"{{"model":"m","stream":{stream},"max_tokens":100,"messages":[{{"role":"user","content":"hi"}}]}}"#
        )
    };
    let (status, head, body) = http(&port, &completions, &[&auth], Some(&req(true)));
    assert_eq!(status, 200, "{body}");
    assert!(
        head.contains("x-potluck-provider: bob") && head.contains("text/event-stream"),
        "{head}"
    );
    assert!(
        body.contains("\"Hel\"") && body.contains("[DONE]"),
        "{body}"
    );
    assert!(seen.recv().unwrap().contains("\"max_tokens\":5"));
    let (status, _, body) = http(&port, &completions, &[&auth], Some(&req(false)));
    assert_eq!(status, 200);
    assert!(body.contains("\"content\":\"Hello\""), "{body}");
    seen.recv().unwrap();

    // Unknown model: nobody serves it.
    let other = r#"{"model":"nope","messages":[]}"#;
    assert_eq!(http(&port, &completions, &[&auth], Some(other)).0, 503);

    // The REPL asks too, and bob logs who used his GPU.
    alice.say("/ask m hi there");
    alice.expect("<m@bob> Hello");
    seen.recv().unwrap();
    let log = std::fs::read_to_string(root.join("bob/requests.log")).unwrap();
    assert_eq!(
        log.lines().filter(|l| l.ends_with(" alice m 200")).count(),
        3,
        "{log}"
    );

    // Bob blocks alice: his GPU refuses her.
    bob.say("/block alice");
    bob.expect("* alice blocked");
    assert_eq!(
        http(&port, &completions, &[&auth], Some(&req(false))).0,
        403
    );

    let _ = std::fs::remove_dir_all(root);
}
