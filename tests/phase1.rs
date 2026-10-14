//! Phase 1 exit check: 5 people chat in one room; a removed member is locked out.

use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

struct Peer {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
    /// Lines seen; `log[..used]` were consumed by `expect`.
    log: Vec<String>,
    used: usize,
}

impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn potluck(dir: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_potluck"));
    cmd.arg("--dir").arg(dir);
    cmd
}

impl Peer {
    fn spawn(dir: &Path, args: &[&str]) -> Self {
        let mut child = potluck(dir)
            .args(args)
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
            log: Vec::new(),
            used: 0,
        }
    }

    fn say(&mut self, line: &str) {
        writeln!(self.stdin, "{line}").unwrap();
    }

    /// Waits for an unconsumed line containing `pat` and consumes it.
    fn expect(&mut self, pat: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(i) = (self.used..self.log.len()).find(|&i| self.log[i].contains(pat)) {
                self.log.swap(self.used, i);
                self.used += 1;
                return self.log[self.used - 1].clone();
            }
            match self
                .lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(l) => self.log.push(l),
                Err(_) => break,
            }
        }
        panic!("timed out waiting for {pat:?}; saw {:#?}", self.log);
    }

    /// Collects whatever arrives within `d`.
    fn settle(&mut self, d: Duration) {
        let deadline = Instant::now() + d;
        while let Ok(l) = self
            .lines
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            self.log.push(l);
        }
    }

    fn saw(&self, pat: &str) -> bool {
        self.log.iter().any(|l| l.contains(pat))
    }
}

#[test]
fn five_chat_and_removed_member_is_locked_out() {
    let root: PathBuf = std::env::temp_dir().join(format!("potluck-p1-{}", std::process::id()));
    let dir = |nick: &str| root.join(nick);

    let out = potluck(&dir("alice"))
        .args(["create", "friends", "--nick", "alice"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let mut room = vec![Peer::spawn(&dir("alice"), &["chat", "friends"])];
    room[0].expect("* listening /ip4/127.0.0.1/tcp/");

    for nick in ["bob", "carol", "dave", "erin"] {
        room[0].say("/invite");
        let ticket = room[0]
            .expect("* invite ")
            .split_whitespace()
            .nth(3)
            .unwrap()
            .to_owned();
        let mut p = Peer::spawn(&dir(nick), &["join", &ticket, "--nick", nick]);
        p.expect(&format!("* in friends as {nick}"));
        for m in &mut room {
            m.expect(&format!("* {nick} joined"));
        }
        room.push(p);
    }

    // A used ticket doesn't admit anyone else.
    room[0].say("/invite");
    let ticket = room[0]
        .expect("* invite ")
        .split_whitespace()
        .nth(3)
        .unwrap()
        .to_owned();
    let mut mallory = Peer::spawn(&dir("mallory"), &["join", &ticket, "--nick", "mallory"]);
    mallory.expect("* in friends as mallory");
    let reuse = potluck(&dir("eve"))
        .args(["join", &ticket, "--nick", "eve"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&reuse.stderr).contains("invite used up"));
    room[0].say("/remove mallory");
    drop(mallory);

    // Everyone hears everyone.
    let nicks = ["alice", "bob", "carol", "dave", "erin"];
    for (i, nick) in nicks.iter().enumerate() {
        room[i].say(&format!("hi from {nick}"));
    }
    for (i, m) in room.iter_mut().enumerate() {
        for (j, nick) in nicks.iter().enumerate() {
            if i != j {
                m.expect(&format!("<{nick}> hi from {nick}"));
            }
        }
    }

    // Remove bob: he can neither post nor read.
    room[0].say("/remove bob");
    for i in [0, 2, 3, 4] {
        room[i].expect("* bob removed");
    }
    room[1].say("ghost");
    room[2].say("after");
    for i in [0, 3, 4] {
        room[i].expect("<carol> after");
    }
    for m in &mut room {
        m.settle(Duration::from_secs(1));
    }
    assert!(
        !room.iter().any(|m| m.saw("<bob> ghost")),
        "removed member's message delivered"
    );
    assert!(
        !room[1].saw("<carol> after"),
        "removed member still reads the room"
    );

    let _ = std::fs::remove_dir_all(root);
}
