use futures_util::StreamExt;
use p2panda::{
    Hash, Topic,
    streams::{EphemeralStreamPublisher, EphemeralStreamSubscription},
};
use p2panda_encryption::crypto::{
    hpke::{HpkeCiphertext, hpke_open, hpke_seal},
    x25519::{PublicKey, SecretKey},
};
use pepino_core::{BoardId, DeviceId};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

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
    Carrying,
    NotTrusted,
    AtCapacity,
    RemovalPendingRestart,
    Removed,
    Superseded,
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
        body: &impl Serialize,
    ) -> Result<Self, Error> {
        let key = control_public_key(to)?;
        let aad = associated_data(author, to, challenge, direction);
        let bytes = p2panda_core::cbor::encode_cbor(body).map_err(|_| control_error())?;
        let sealed =
            hpke_seal(&key, Some(CONTEXT), Some(&aad), &bytes).map_err(|_| control_error())?;
        Ok(Self {
            to,
            challenge,
            direction,
            sealed,
        })
    }

    fn open<T: DeserializeOwned>(&self, author: DeviceId, keys: &DeviceCredentials) -> Option<T> {
        if self.to != keys.device_id() {
            return None;
        }
        let aad = associated_data(author, self.to, self.challenge, self.direction);
        let secret = p2panda_spaces::Credentials::from(&keys.0).signing_key();
        let scalar = ed25519_dalek::SigningKey::from_bytes(secret.as_bytes()).to_scalar_bytes();
        let key = SecretKey::from_bytes(scalar);
        let bytes = hpke_open(&self.sealed, &key, Some(CONTEXT), Some(&aad)).ok()?;
        p2panda_core::cbor::decode_cbor_strict(&bytes[..]).ok()
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
}

pub(crate) struct IncomingInstruction {
    pub author: DeviceId,
    pub challenge: [u8; 16],
    envelope: Envelope,
}

impl IncomingInstruction {
    pub fn open(&self, keys: &DeviceCredentials) -> Option<RelayInstruction> {
        self.envelope.open(self.author, keys)
    }
}

impl Node {
    pub async fn relay_channel(&self, relay: DeviceId) -> Result<RelayChannel, Error> {
        let node = self.inner.clone();
        let id = self.device_id();
        self.worker
            .run(async move {
                let mut topic = CONTEXT.to_vec();
                topic.extend_from_slice(relay.as_bytes());
                let topic = Topic::from(Hash::digest(topic));
                let (tx, rx) = node.ephemeral_stream(topic).await.map_err(Error::node)?;
                Ok(RelayChannel { id, tx, rx })
            })
            .await
    }
}

impl RelayChannel {
    pub async fn send(
        &self,
        relay: DeviceId,
        challenge: [u8; 16],
        instruction: RelayInstruction,
    ) -> Result<(), Error> {
        self.send_message(relay, challenge, Direction::Instruction, &instruction)
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
            &answer,
        )
        .await
    }

    async fn send_message(
        &self,
        to: DeviceId,
        challenge: [u8; 16],
        direction: Direction,
        body: &impl Serialize,
    ) -> Result<(), Error> {
        let envelope = Envelope::seal(self.id, to, challenge, direction, body)?;
        self.tx.publish(envelope).await.map_err(Error::node)
    }

    pub async fn receive_answer(
        &mut self,
        relay: DeviceId,
        keys: &DeviceCredentials,
    ) -> Option<([u8; 16], RelayAnswer)> {
        while let Some(message) = self.rx.next().await {
            let author = DeviceId::from_bytes(*message.author().as_bytes());
            let body = message.body();
            if author == relay
                && body.direction == Direction::Answer
                && let Some(answer) = body.open(author, keys)
            {
                return Some((body.challenge, answer));
            }
        }
        None
    }

    pub(crate) async fn receive_instruction(&mut self) -> Option<IncomingInstruction> {
        while let Some(message) = self.rx.next().await {
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
    let envelope = Envelope::seal(
        device,
        relay.device_id(),
        [7; 16],
        Direction::Instruction,
        &instruction,
    )
    .unwrap();
    let bytes = p2panda_core::cbor::encode_cbor(&envelope).unwrap();
    assert!(!bytes.windows(32).any(|part| part == board.as_bytes()));
    let mut decoded: Envelope = p2panda_core::cbor::decode_cbor(&bytes[..]).unwrap();
    assert_eq!(
        decoded.open::<RelayInstruction>(device, &relay),
        Some(instruction)
    );
    assert_eq!(
        decoded.open::<RelayInstruction>(stranger.device_id(), &relay),
        None
    );
    assert_eq!(decoded.open::<RelayInstruction>(device, &stranger), None);
    decoded.challenge[0] ^= 1;
    assert_eq!(decoded.open::<RelayInstruction>(device, &relay), None);
    decoded.challenge[0] ^= 1;
    decoded.direction = Direction::Answer;
    assert_eq!(decoded.open::<RelayInstruction>(device, &relay), None);
}
