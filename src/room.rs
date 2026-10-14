//! Room membership: an order-free set of signed events every member merges.

use std::{
    collections::{BTreeMap, BTreeSet},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use libp2p::{
    Multiaddr, PeerId,
    identity::{Keypair, PublicKey},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Event {
    Genesis {
        name: String,
        nick: String,
        nonce: u64,
    },
    Invite {
        room: String,
        invitee: Option<PeerId>,
        uses: u32,
        expires: u64,
        nonce: u64,
    },
    Join {
        room: String,
        nick: String,
        invite: Signed,
    },
    Revoke {
        room: String,
        invite: String,
    },
    Remove {
        room: String,
        join: String,
    },
    Chat {
        room: String,
        text: String,
        nonce: u64,
    },
}

/// A JSON event body signed by `by`. Its id is the hash of the exact body bytes.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Signed {
    pub body: String,
    pub by: PeerId,
    pub sig: String,
}

impl Signed {
    pub fn new(key: &Keypair, event: &Event) -> Result<Self> {
        let body = serde_json::to_string(event)?;
        let sig = B64.encode(key.sign(body.as_bytes())?);
        Ok(Self {
            body,
            by: key.public().to_peer_id(),
            sig,
        })
    }

    pub fn id(&self) -> String {
        B64.encode(Sha256::digest(self.body.as_bytes()))
    }

    /// Verifies the signature and parses the body.
    pub fn open(&self) -> Result<Event> {
        // ed25519 peer IDs embed the public key.
        let key = PublicKey::try_decode_protobuf(self.by.as_ref().digest())
            .context("signer is not an ed25519 key")?;
        ensure!(
            key.verify(self.body.as_bytes(), &B64.decode(&self.sig)?),
            "bad signature"
        );
        Ok(serde_json::from_str(&self.body)?)
    }

    /// Parses a body that was verified when it entered the room.
    fn event(&self) -> Event {
        serde_json::from_str(&self.body).expect("verified on insert")
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Unique per sender: two messages never share a body, so never share an id.
pub fn nonce() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

#[derive(Serialize, Deserialize)]
pub struct Room {
    pub id: String,
    pub events: BTreeMap<String, Signed>,
    /// Known addresses, each ending in /p2p/<peer>.
    pub peers: BTreeSet<Multiaddr>,
}

impl Room {
    pub fn create(key: &Keypair, name: &str, nick: &str) -> Result<Self> {
        let genesis = Signed::new(
            key,
            &Event::Genesis {
                name: name.into(),
                nick: nick.into(),
                nonce: nonce(),
            },
        )?;
        Self::from_genesis(genesis)
    }

    pub fn from_genesis(genesis: Signed) -> Result<Self> {
        ensure!(
            matches!(genesis.open()?, Event::Genesis { .. }),
            "not a genesis event"
        );
        let id = genesis.id();
        Ok(Self {
            id: id.clone(),
            events: BTreeMap::from([(id, genesis)]),
            peers: BTreeSet::new(),
        })
    }

    pub fn owner(&self) -> PeerId {
        self.events[&self.id].by
    }

    pub fn name(&self) -> String {
        match self.events[&self.id].event() {
            Event::Genesis { name, .. } => name,
            _ => unreachable!(),
        }
    }

    /// Validates and stores an event; `Ok(false)` if already known.
    pub fn add(&mut self, s: Signed) -> Result<bool> {
        let id = s.id();
        if self.events.contains_key(&id) {
            return Ok(false);
        }
        self.check(&s)?;
        self.events.insert(id, s);
        Ok(true)
    }

    /// Order-independent validity: any member reaches the same verdict at any time.
    fn check(&self, s: &Signed) -> Result<()> {
        let owner = self.owner();
        match s.open()? {
            Event::Invite { room, .. }
            | Event::Revoke { room, .. }
            | Event::Remove { room, .. } => {
                ensure!(room == self.id, "wrong room");
                ensure!(s.by == owner, "only the owner signs this");
            }
            Event::Join { room, invite, .. } => {
                ensure!(room == self.id, "wrong room");
                let Event::Invite { room, invitee, .. } = invite.open()? else {
                    bail!("join without invite")
                };
                ensure!(
                    room == self.id && invite.by == owner,
                    "invite not from this room's owner"
                );
                ensure!(
                    invitee.is_none_or(|k| k == s.by),
                    "invite is for someone else"
                );
            }
            Event::Genesis { .. } | Event::Chat { .. } => bail!("not a room event"),
        }
        Ok(())
    }

    fn joins(&self) -> impl Iterator<Item = (&String, &Signed, Event)> {
        self.events.iter().filter_map(|(id, s)| match s.event() {
            e @ Event::Join { .. } => Some((id, s, e)),
            _ => None,
        })
    }

    fn named(&self, f: impl Fn(Event) -> Option<String>) -> BTreeSet<String> {
        self.events.values().filter_map(|s| f(s.event())).collect()
    }

    /// Members and their nicks: owner + joins not named by a `remove`.
    pub fn members(&self) -> BTreeMap<PeerId, String> {
        let removed = self.named(|e| match e {
            Event::Remove { join, .. } => Some(join),
            _ => None,
        });
        let mut members = BTreeMap::new();
        if let Event::Genesis { nick, .. } = self.events[&self.id].event() {
            members.insert(self.owner(), nick);
        }
        for (id, s, e) in self.joins() {
            if let Event::Join { nick, .. } = e
                && !removed.contains(id)
            {
                members.insert(s.by, nick);
            }
        }
        members
    }

    pub fn is_member(&self, peer: &PeerId) -> bool {
        self.members().contains_key(peer)
    }

    /// Door checks the admitting member runs on top of `check`.
    pub fn admit(&self, join: &Signed) -> Result<()> {
        let Event::Join { invite, .. } = join.open()? else {
            bail!("not a join")
        };
        let Event::Invite {
            invitee,
            uses,
            expires,
            ..
        } = invite.open()?
        else {
            bail!("join without invite")
        };
        let invite_id = invite.id();
        ensure!(now() < expires, "invite expired");
        let revoked = self.named(|e| match e {
            Event::Revoke { invite, .. } => Some(invite),
            _ => None,
        });
        ensure!(!revoked.contains(&invite_id), "invite revoked");
        let used = self
            .joins()
            .filter(|(_, _, e)| matches!(e, Event::Join { invite, .. } if invite.id() == invite_id))
            .count();
        ensure!(used < uses as usize, "invite used up");
        let was_member = self.joins().any(|(_, s, _)| s.by == join.by);
        ensure!(
            !was_member || invitee == Some(join.by),
            "removed members need an invite naming them"
        );
        self.check(join)
    }

    /// Owner-signed `remove` events for each live join of `peer`.
    pub fn remove(&self, key: &Keypair, peer: &PeerId) -> Result<Vec<Signed>> {
        let removed = self.named(|e| match e {
            Event::Remove { join, .. } => Some(join),
            _ => None,
        });
        self.joins()
            .filter(|(id, s, _)| s.by == *peer && !removed.contains(*id))
            .map(|(id, _, _)| {
                Signed::new(
                    key,
                    &Event::Remove {
                        room: self.id.clone(),
                        join: id.clone(),
                    },
                )
            })
            .collect()
    }
}

/// What an invitee needs: the invite plus addresses to reach a member.
#[derive(Serialize, Deserialize)]
pub struct Ticket {
    pub invite: Signed,
    pub peers: Vec<Multiaddr>,
}

impl Ticket {
    pub fn encode(&self) -> String {
        B64.encode(serde_json::to_vec(self).unwrap())
    }

    pub fn decode(s: &str) -> Result<Self> {
        Ok(serde_json::from_slice(
            &B64.decode(s.trim()).context("bad ticket")?,
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invite(
        owner: &Keypair,
        room: &Room,
        invitee: Option<PeerId>,
        uses: u32,
        expires: u64,
    ) -> Signed {
        let e = Event::Invite {
            room: room.id.clone(),
            invitee,
            uses,
            expires,
            nonce: nonce(),
        };
        Signed::new(owner, &e).unwrap()
    }

    fn join(key: &Keypair, room: &Room, nick: &str, invite: &Signed) -> Signed {
        let e = Event::Join {
            room: room.id.clone(),
            nick: nick.into(),
            invite: invite.clone(),
        };
        Signed::new(key, &e).unwrap()
    }

    #[test]
    fn membership() {
        let (owner, bob, eve) = (
            Keypair::generate_ed25519(),
            Keypair::generate_ed25519(),
            Keypair::generate_ed25519(),
        );
        let mut room = Room::create(&owner, "r", "alice").unwrap();
        let bob_id = bob.public().to_peer_id();

        // Door: single use, expiry, forged invites.
        let inv = invite(&owner, &room, None, 1, now() + 60);
        let j = join(&bob, &room, "bob", &inv);
        room.admit(&j).unwrap();
        assert!(room.add(j.clone()).unwrap());
        assert!(!room.add(j).unwrap());
        assert!(
            room.admit(&join(&eve, &room, "eve", &inv)).is_err(),
            "used up"
        );
        assert!(
            room.admit(&join(
                &eve,
                &room,
                "eve",
                &invite(&owner, &room, None, 1, now() - 1)
            ))
            .is_err(),
            "expired"
        );
        assert!(
            room.add(join(
                &eve,
                &room,
                "eve",
                &invite(&eve, &room, None, 1, now() + 60)
            ))
            .is_err(),
            "forged"
        );
        assert_eq!(room.members().len(), 2);

        // Only the owner removes; removal sticks across merge order.
        let rm = room.remove(&owner, &bob_id).unwrap();
        assert!(
            room.add(room.remove(&bob, &bob_id).unwrap()[0].clone())
                .is_err()
        );
        let mut other = Room::from_genesis(room.events[&room.id].clone()).unwrap();
        for s in rm.iter().chain(room.events.values()) {
            other.add(s.clone()).ok();
        }
        room.add(rm[0].clone()).unwrap();
        assert!(!room.is_member(&bob_id) && !other.is_member(&bob_id));

        // A removed key needs an invite naming it.
        assert!(
            room.admit(&join(
                &bob,
                &room,
                "bob",
                &invite(&owner, &room, None, 5, now() + 60)
            ))
            .is_err()
        );
        let j = join(
            &bob,
            &room,
            "bob",
            &invite(&owner, &room, Some(bob_id), 1, now() + 60),
        );
        room.admit(&j).unwrap();
        room.add(j).unwrap();
        assert!(room.is_member(&bob_id));
    }
}
