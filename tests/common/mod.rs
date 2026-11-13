#![allow(dead_code)] // each test binary uses a different subset

use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub struct Peer {
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
        let _ = self.child.wait(); // gone before a test touches its files
    }
}

pub fn potluck(dir: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_potluck"));
    cmd.arg("--dir").arg(dir);
    cmd
}

impl Peer {
    pub fn spawn(dir: &Path, args: &[&str]) -> Self {
        let mut child = potluck(dir)
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
            log: Vec::new(),
            used: 0,
        }
    }

    pub fn say(&mut self, line: &str) {
        writeln!(self.stdin, "{line}").unwrap();
    }

    /// Waits up to `d` for an unconsumed line containing `pat` and consumes it.
    pub fn try_expect(&mut self, pat: &str, d: Duration) -> Option<String> {
        let deadline = Instant::now() + d;
        loop {
            if let Some(i) = (self.used..self.log.len()).find(|&i| self.log[i].contains(pat)) {
                let l = self.log.remove(i);
                self.log.insert(self.used, l);
                self.used += 1;
                return Some(self.log[self.used - 1].clone());
            }
            match self
                .lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(l) => self.log.push(l),
                Err(_) => return None,
            }
        }
    }

    /// Like `try_expect` with a 20 s limit, but panics on timeout.
    pub fn expect(&mut self, pat: &str) -> String {
        self.try_expect(pat, Duration::from_secs(20))
            .unwrap_or_else(|| panic!("timed out waiting for {pat:?}; saw {:#?}", self.log))
    }

    /// Collects whatever arrives within `d`.
    pub fn settle(&mut self, d: Duration) {
        let deadline = Instant::now() + d;
        while let Ok(l) = self
            .lines
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            self.log.push(l);
        }
    }

    pub fn saw(&self, pat: &str) -> bool {
        self.log.iter().any(|l| l.contains(pat))
    }
}
