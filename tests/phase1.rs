//! Phase 1 exit check: 5 people chat in one room; a removed member is locked out.

mod common;

use common::{potluck, Peer};
use std::{path::PathBuf, time::Duration};

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
    let alice_addr = room[0]
        .expect("* listening /ip4/127.0.0.1/tcp/")
        .trim_start_matches("* listening ")
        .trim()
        .to_owned();

    let mut erin_ticket = String::new();
    for nick in ["bob", "carol", "dave", "erin"] {
        room[0].say("/invite");
        let ticket = room[0]
            .expect("* invite ")
            .split_whitespace()
            .nth(3)
            .unwrap()
            .to_owned();
        // Guards: a joiner dialing a relay that is itself a member (alice) must still reach the room.
        let mut args = vec!["join", ticket.as_str(), "--nick", nick];
        if nick == "erin" {
            args.extend(["--relay", alice_addr.as_str()]);
            erin_ticket = ticket.clone();
        }
        let mut p = Peer::spawn(&dir(nick), &args);
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

    // A lost join reply: erin loses her room file and gets back in with the same ticket.
    // Last, because members may keep her dead connection for a while and miss-send to it.
    drop(room.pop());
    std::fs::remove_dir_all(dir("erin").join("rooms")).unwrap();
    Peer::spawn(&dir("erin"), &["join", &erin_ticket, "--nick", "erin"])
        .expect("* in friends as erin");

    let _ = std::fs::remove_dir_all(root);
}
