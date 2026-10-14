//! Phase 0 exit check: a completion streams requester → relay → provider → backend and back.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::Path,
    process::{Child, Command, Output, Stdio},
    sync::mpsc,
    thread,
};

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

fn potluck(key: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_potluck"));
    cmd.arg("--key").arg(key);
    cmd
}

/// Spawns a long-running node and returns the first stdout line starting with `prefix`.
fn spawn_until(key: &Path, args: &[&str], prefix: &str) -> (Kill, String) {
    let mut child = potluck(key)
        .args(args)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let prefix = prefix.to_owned();
    // Keep draining: a closed pipe would make the node's next println! panic.
    thread::spawn(move || {
        for l in BufReader::new(stdout).lines().map_while(Result::ok) {
            if l.starts_with(&prefix) && l.contains("/127.0.0.1/tcp/") {
                let _ = tx.send(l[prefix.len()..].to_owned());
            }
        }
    });
    (Kill(child), rx.recv().expect("node exited early"))
}

/// One-shot OpenAI-style backend that streams "Hello" and hands back the request it saw.
fn fake_backend() -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut conn, _) = listener.accept().unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0; 4096];
        while !String::from_utf8_lossy(&buf).contains("\"messages\"") {
            let n = conn.read(&mut chunk).unwrap();
            buf.extend_from_slice(&chunk[..n]);
        }
        let mut sse = String::new();
        for t in ["Hel", "lo"] {
            sse += &format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{t}\"}}}}]}}\n\n");
        }
        sse += "data: [DONE]\n\n";
        write!(
            conn,
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{sse}"
        )
        .unwrap();
        tx.send(String::from_utf8(buf).unwrap()).unwrap();
    });
    (addr, rx)
}

fn ask(key: &Path, peer: &str, model: &str) -> Output {
    potluck(key)
        .args(["ask", "--peer", peer, "--model", model, "hi"])
        .output()
        .unwrap()
}

#[test]
fn completion_streams_via_relay() {
    let dir = std::env::temp_dir().join(format!("potluck-test-{}", std::process::id()));
    let (backend, seen) = fake_backend();

    let (_relay, relay_addr) =
        spawn_until(&dir.join("relay"), &["relay", "--port", "0"], "listening ");
    let backend_url = format!("http://{backend}");
    let (_provider, provider_addr) = spawn_until(
        &dir.join("provider"),
        &[
            "serve",
            "--relay",
            &relay_addr,
            "--backend",
            &backend_url,
            "--model",
            "m",
        ],
        "serving ",
    );
    assert!(provider_addr.contains("/p2p-circuit/"), "{provider_addr}");

    // Sandbox: unshared models never reach the backend.
    let denied = ask(&dir.join("requester"), &provider_addr, "other");
    assert!(!denied.status.success());
    assert!(String::from_utf8_lossy(&denied.stderr).contains("model not shared"));

    let out = ask(&dir.join("requester"), &provider_addr, "m");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "Hello\n");

    let req = seen.recv().unwrap();
    assert!(req.starts_with("POST /v1/chat/completions "), "{req}");
    assert!(req.contains("\"stream\":true"), "{req}");

    let _ = std::fs::remove_dir_all(dir);
}
