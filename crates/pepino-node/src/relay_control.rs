use std::{future::poll_fn, pin::Pin, time::Duration};

use futures_util::Stream;
use p2panda::{
    Hash, Topic,
    streams::{EphemeralStreamPublisher, EphemeralStreamSubscription},
};
use p2panda_encryption::crypto::{
    hpke::{HpkeCiphertext, hpke_open, hpke_seal},
    x25519::{PublicKey, SecretKey},
};
use pepino_core::{BoardId, DeviceId};
use serde::{Deserialize, Serialize};
use tokio::time::{Interval, MissedTickBehavior, interval};

use crate::{DeviceCredentials, Error, Node};

const CONTEXT: &[u8] = b"pepino relay control v1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct RelayRevision {
    pub sequence: u64,
    pub writer: DeviceId,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RelayInstruction {
    pub board: BoardId,
    pub sequence: u64,
    pub carry: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayAnswer {
    Carrying = 0,
    NotTrusted = 1,
    AtCapacity = 2,
    RemovalPendingRestart = 3,
    Removed = 4,
    Superseded = 5,
}

impl RelayAnswer {
    const ALL: [Self; 6] = [
        Self::Carrying,
        Self::NotTrusted,
        Self::AtCapacity,
        Self::RemovalPendingRestart,
        Self::Removed,
        Self::Superseded,
    ];

    // Every answer seals to the same length, so its ciphertext size does not reveal the outcome.
    fn encode(self) -> [u8; 1] {
        [self as u8]
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let &[code] = bytes else {
            return None;
        };
        Self::ALL.into_iter().find(|answer| *answer as u8 == code)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum Direction {
    Instruction,
    Answer,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Envelope {
    to: DeviceId,
    challenge: [u8; 16],
    direction: Direction,
    sealed: HpkeCiphertext,
}

impl Envelope {
    fn seal(
        author: DeviceId,
        to: DeviceId,
        challenge: [u8; 16],
        direction: Direction,
        bytes: &[u8],
    ) -> Result<Self, Error> {
        let key = control_public_key(to)?;
        let aad = associated_data(author, to, challenge, direction);
        let sealed =
            hpke_seal(&key, Some(CONTEXT), Some(&aad), bytes).map_err(|_| control_error())?;
        Ok(Self {
            to,
            challenge,
            direction,
            sealed,
        })
    }

    fn open(&self, author: DeviceId, keys: &DeviceCredentials) -> Option<Vec<u8>> {
        if self.to != keys.device_id() {
            return None;
        }
        let aad = associated_data(author, self.to, self.challenge, self.direction);
        let secret = p2panda_spaces::Credentials::from(&keys.0).signing_key();
        let scalar = ed25519_dalek::SigningKey::from_bytes(secret.as_bytes()).to_scalar_bytes();
        let key = SecretKey::from_bytes(scalar);
        hpke_open(&self.sealed, &key, Some(CONTEXT), Some(&aad)).ok()
    }
}

pub fn validate_relay_id(id: DeviceId) -> Result<(), Error> {
    control_public_key(id).map(|_| ())
}

fn control_public_key(id: DeviceId) -> Result<PublicKey, Error> {
    let key = ed25519_dalek::VerifyingKey::from_bytes(id.as_bytes())
        .ok()
        .filter(|key| !key.is_weak())
        .ok_or_else(|| Error::Settings("invalid relay control recipient key".into()))?;
    Ok(PublicKey::from_bytes(key.to_montgomery().to_bytes()))
}

fn associated_data(
    author: DeviceId,
    to: DeviceId,
    challenge: [u8; 16],
    direction: Direction,
) -> [u8; 81] {
    let mut aad = [0; 81];
    aad[..32].copy_from_slice(author.as_bytes());
    aad[32..64].copy_from_slice(to.as_bytes());
    aad[64..80].copy_from_slice(&challenge);
    aad[80] = match direction {
        Direction::Instruction => 0,
        Direction::Answer => 1,
    };
    aad
}

fn control_error() -> Error {
    Error::Node("could not encode relay control message".into())
}

pub struct RelayChannel {
    id: DeviceId,
    tx: EphemeralStreamPublisher<Envelope>,
    rx: EphemeralStreamSubscription<Envelope>,
    repoll: Interval,
}

pub(crate) struct IncomingInstruction {
    pub author: DeviceId,
    pub challenge: [u8; 16],
    envelope: Envelope,
}

impl IncomingInstruction {
    pub fn open(&self, keys: &DeviceCredentials) -> Option<RelayInstruction> {
        decode_instruction(&self.envelope.open(self.author, keys)?)
    }
}

fn decode_instruction(bytes: &[u8]) -> Option<RelayInstruction> {
    p2panda_core::cbor::decode_cbor_strict(bytes).ok()
}

impl Node {
    pub async fn relay_channel(&self, relay: DeviceId) -> Result<RelayChannel, Error> {
        let node = self.inner.clone();
        let id = self.device_id();
        self.worker
            .run(async move {
                node.ephemeral_stream(control_topic(relay))
                    .await
                    .map_err(Error::node)
            })
            .await
            .map(|(tx, rx)| {
                let mut repoll = interval(Duration::from_millis(100));
                repoll.set_missed_tick_behavior(MissedTickBehavior::Delay);
                RelayChannel { id, tx, rx, repoll }
            })
    }
}

fn control_topic(relay: DeviceId) -> Topic {
    let mut topic = CONTEXT.to_vec();
    topic.extend_from_slice(relay.as_bytes());
    Topic::from(Hash::digest(topic))
}

impl RelayChannel {
    async fn next(&mut self) -> Option<p2panda::streams::EphemeralMessage<Envelope>> {
        poll_fn(|cx| {
            let item = Pin::new(&mut self.rx).poll_next(cx);
            // p2panda 0.8.1 may return Pending without a waker after discarding an item.
            if item.is_pending() && self.repoll.poll_tick(cx).is_ready() {
                cx.waker().wake_by_ref();
            }
            item
        })
        .await
    }

    pub async fn send(
        &self,
        relay: DeviceId,
        challenge: [u8; 16],
        instruction: RelayInstruction,
    ) -> Result<(), Error> {
        let bytes = p2panda_core::cbor::encode_cbor(&instruction).map_err(|_| control_error())?;
        self.send_message(relay, challenge, Direction::Instruction, &bytes)
            .await
    }

    pub(crate) async fn answer(
        &self,
        incoming: &IncomingInstruction,
        answer: RelayAnswer,
    ) -> Result<(), Error> {
        self.send_message(
            incoming.author,
            incoming.challenge,
            Direction::Answer,
            &answer.encode(),
        )
        .await
    }

    async fn send_message(
        &self,
        to: DeviceId,
        challenge: [u8; 16],
        direction: Direction,
        bytes: &[u8],
    ) -> Result<(), Error> {
        let envelope = Envelope::seal(self.id, to, challenge, direction, bytes)?;
        self.tx.publish(envelope).await.map_err(Error::node)
    }

    pub async fn receive_answer(
        &mut self,
        relay: DeviceId,
        keys: &DeviceCredentials,
    ) -> Option<([u8; 16], RelayAnswer)> {
        while let Some(message) = self.next().await {
            let author = DeviceId::from_bytes(*message.author().as_bytes());
            let body = message.body();
            if author == relay
                && body.direction == Direction::Answer
                && let Some(answer) = body
                    .open(author, keys)
                    .and_then(|bytes| RelayAnswer::decode(&bytes))
            {
                return Some((body.challenge, answer));
            }
        }
        None
    }

    pub(crate) async fn receive_instruction(&mut self) -> Option<IncomingInstruction> {
        while let Some(message) = self.next().await {
            let body = message.body();
            if body.to == self.id && body.direction == Direction::Instruction {
                return Some(IncomingInstruction {
                    author: DeviceId::from_bytes(*message.author().as_bytes()),
                    challenge: body.challenge,
                    envelope: body.clone(),
                });
            }
        }
        None
    }
}

#[cfg(test)]
#[test]
fn instruction_encoding_binds_sender_recipient_direction_and_challenge() {
    let dir = tempfile::tempdir().unwrap();
    let relay = DeviceCredentials::load_or_create(dir.path().join("relay.cbor")).unwrap();
    let stranger = DeviceCredentials::load_or_create(dir.path().join("stranger.cbor")).unwrap();
    let device = DeviceId::from_bytes([42; 32]);
    let board = BoardId::from_bytes([71; 32]);
    let instruction = RelayInstruction {
        board,
        sequence: 49,
        carry: true,
    };
    let instruction_bytes = p2panda_core::cbor::encode_cbor(&instruction).unwrap();
    let envelope = Envelope::seal(
        device,
        relay.device_id(),
        [7; 16],
        Direction::Instruction,
        &instruction_bytes,
    )
    .unwrap();
    let bytes = p2panda_core::cbor::encode_cbor(&envelope).unwrap();
    assert!(!bytes.windows(32).any(|part| part == board.as_bytes()));
    let mut decoded: Envelope = p2panda_core::cbor::decode_cbor(&bytes[..]).unwrap();
    assert_eq!(
        decoded
            .open(device, &relay)
            .and_then(|bytes| decode_instruction(&bytes)),
        Some(instruction)
    );
    assert_eq!(decoded.open(stranger.device_id(), &relay), None);
    assert_eq!(decoded.open(device, &stranger), None);
    decoded.challenge[0] ^= 1;
    assert_eq!(decoded.open(device, &relay), None);
    decoded.challenge[0] ^= 1;
    decoded.direction = Direction::Answer;
    assert_eq!(decoded.open(device, &relay), None);
}

#[cfg(test)]
#[test]
fn all_answers_have_the_same_encrypted_length() {
    let dir = tempfile::tempdir().unwrap();
    let relay = DeviceCredentials::load_or_create(dir.path().join("relay.cbor")).unwrap();
    let device = DeviceCredentials::load_or_create(dir.path().join("device.cbor")).unwrap();
    let lengths: std::collections::BTreeSet<_> = RelayAnswer::ALL
        .into_iter()
        .map(|answer| {
            let envelope = Envelope::seal(
                relay.device_id(),
                device.device_id(),
                [17; 16],
                Direction::Answer,
                &answer.encode(),
            )
            .unwrap();
            assert_eq!(
                envelope
                    .open(relay.device_id(), &device)
                    .and_then(|bytes| RelayAnswer::decode(&bytes)),
                Some(answer)
            );
            (
                envelope.sealed.ciphertext.len(),
                p2panda_core::cbor::encode_cbor(&envelope).unwrap().len(),
            )
        })
        .collect();
    assert_eq!(lengths.len(), 1);
    for malformed in [&[][..], &[0, 0], &[RelayAnswer::ALL.len() as u8], &[255]] {
        assert_eq!(RelayAnswer::decode(malformed), None);
    }
}

#[cfg(test)]
#[tokio::test]
#[cfg_attr(not(pepino_network_tests), ignore = "requires PEPINO_NETWORK_TESTS=1")]
async fn receives_valid_instruction_after_junk_on_loopback() {
    use tokio::{sync::mpsc, time::timeout};

    let dir = tempfile::tempdir().unwrap();
    let network = format!(
        "relay-control-{}",
        dir.path().file_name().unwrap().to_string_lossy()
    );
    let settings = |name: &str| {
        let home = dir.path().join(name);
        std::fs::create_dir(&home).unwrap();
        let keys = DeviceCredentials::load_or_create(home.join("device.cbor")).unwrap();
        let mut settings = crate::NodeSettings::new(keys, home.join("node.sqlite"));
        settings.network_name = network.clone();
        settings.bind_ip_v4 = Some(std::net::Ipv4Addr::LOCALHOST);
        settings
    };
    let relay = Node::spawn(settings("relay")).await.unwrap();
    let device = Node::spawn(settings("device")).await.unwrap();
    let relay_id = relay.device_id();
    let mut recipient = relay.relay_channel(relay_id).await.unwrap();
    let sender = device.relay_channel(relay_id).await.unwrap();
    let inner = device.inner.clone();
    let (junk, _keep) = device
        .worker
        .run(async move {
            inner
                .ephemeral_stream::<String>(control_topic(relay_id))
                .await
                .map_err(Error::node)
        })
        .await
        .unwrap();
    let (seen, mut received) = mpsc::unbounded_channel();
    let receiver = tokio::spawn(async move {
        while let Some(incoming) = recipient.receive_instruction().await {
            if seen.send(incoming.challenge).is_err() {
                break;
            }
        }
    });
    let instruction = RelayInstruction {
        board: BoardId::from_bytes([71; 32]),
        sequence: 49,
        carry: true,
    };
    let wait = Duration::from_secs(60);
    timeout(wait, async {
        let mut retry = interval(Duration::from_millis(250));
        loop {
            tokio::select! {
                _ = retry.tick() => sender.send(relay_id, [1; 16], instruction).await.unwrap(),
                challenge = received.recv() => {
                    assert_eq!(challenge, Some([1; 16]));
                    break;
                }
            }
        }
    })
    .await
    .expect("relay and device did not connect");

    junk.publish("not a relay envelope".into()).await.unwrap();
    sender.send(relay_id, [2; 16], instruction).await.unwrap();
    let next = timeout(wait, async {
        while let Some(challenge) = received.recv().await {
            if challenge == [2; 16] {
                return Some(challenge);
            }
        }
        None
    })
    .await;
    receiver.abort();
    let _ = receiver.await;
    device.shutdown().await.unwrap();
    relay.shutdown().await.unwrap();
    assert_eq!(
        next.expect("control reception stalled after junk"),
        Some([2; 16])
    );
}
