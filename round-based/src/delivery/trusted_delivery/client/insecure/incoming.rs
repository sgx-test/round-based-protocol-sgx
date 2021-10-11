use std::pin::Pin;
use std::task::{Context, Poll};

use generic_array::{typenum::Unsigned, GenericArray};

use futures::{ready, Stream};
use tokio::io::{self, AsyncRead};

use thiserror::Error;

use crate::delivery::trusted_delivery::client::identity_resolver::IdentityResolver;
use crate::delivery::trusted_delivery::client::insecure::crypto::{DecryptionKey, DecryptionKeys};
use crate::delivery::trusted_delivery::messages::{
    ForwardMsg, InvalidForwardMsg, InvalidForwardMsgHeader, ReceiveData, ReceiveDataError,
};
use crate::Incoming;

pub struct Incomings<P, K, IO> {
    parties: P,
    decryption_keys: K,

    receive: ReceiveData<ForwardMsg, IO>,

    received_valid_message_from: Option<u16>,
    received_message_was_encrypted: bool,
}

#[derive(Debug, Error)]
pub enum ReceiveError {
    #[error("i/o error")]
    Io(
        #[from]
        #[source]
        io::Error,
    ),
    #[error("received invalid message")]
    InvalidMessage(
        #[source]
        #[from]
        InvalidMessage,
    ),
    #[error("internal bug")]
    Bug(
        #[source]
        #[from]
        Bug,
    ),
}

#[derive(Debug, Error)]
pub enum InvalidMessage {
    #[error("invalid header")]
    Header(#[source] InvalidForwardMsgHeader),
    #[error("invalid data")]
    Data(#[source] InvalidForwardMsg),
    #[error("message is too large: size={len}, limit={limit}")]
    TooLarge { len: usize, limit: usize },
    #[error("cannot decrypt the message")]
    Decryption,
}

#[derive(Debug, Error)]
pub enum Bug {
    #[error(".received() returned None though we just received a valid message")]
    ReceivedNone,
}

impl From<io::ErrorKind> for ReceiveError {
    fn from(kind: io::ErrorKind) -> Self {
        Self::Io(kind.into())
    }
}

impl From<ReceiveDataError<InvalidForwardMsgHeader, InvalidForwardMsg>> for ReceiveError {
    fn from(error: ReceiveDataError<InvalidForwardMsgHeader, InvalidForwardMsg>) -> Self {
        match error {
            ReceiveDataError::Io(error) => Self::Io(error),
            ReceiveDataError::ParseHeader(error) => {
                Self::InvalidMessage(InvalidMessage::Header(error))
            }
            ReceiveDataError::ValidateData(error) => {
                Self::InvalidMessage(InvalidMessage::Data(error))
            }
            ReceiveDataError::TooLargeMessage { len, limit } => {
                Self::InvalidMessage(InvalidMessage::TooLarge { len, limit })
            }
        }
    }
}

impl<P, K, IO> Incomings<P, K, IO>
where
    IO: AsyncRead + Unpin,
    P: IdentityResolver + Unpin,
    K: DecryptionKeys + Unpin,
{
    pub fn new(parties: P, decryption_keys: K, receive: ReceiveData<ForwardMsg, IO>) -> Self {
        Self {
            parties,
            decryption_keys,

            receive,

            received_valid_message_from: None,
            received_message_was_encrypted: false,
        }
    }

    pub fn received(&self) -> Option<Incoming<&[u8]>> {
        let sender = match self.received_valid_message_from {
            Some(sender) => sender,
            None => return None,
        };
        let mut data = self
            .receive
            .received()
            .expect("inconsistent internal state")
            .1;
        if self.received_message_was_encrypted {
            // We need to strip the tag
            data = &data[..data.len() - <K::Key as DecryptionKey>::TagSize::USIZE]
        }
        Some(Incoming { sender, msg: data })
    }
}

impl<P, K, IO> Stream for Incomings<P, K, IO>
where
    IO: AsyncRead + Unpin,
    P: IdentityResolver + Unpin,
    K: DecryptionKeys + Unpin,
{
    type Item = Result<(), ReceiveError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        if this.received_valid_message_from.is_some() {
            this.received_valid_message_from = None;
            this.received_message_was_encrypted = false;
        }
        loop {
            match ready!(Pin::new(&mut this.receive).poll_next(cx)) {
                Some(result) => result?,
                None => return Poll::Ready(None),
            }
            let (header, data) = this.receive.received_mut().ok_or(Bug::ReceivedNone)?;
            let sender = match this.parties.lookup_party_index(&header.sender) {
                Some(sender) => sender,
                None => {
                    // Sender is unknown, ignore this message
                    continue;
                }
            };

            if !header.is_broadcast {
                if let Some(decryption_key) =
                    this.decryption_keys.get_decryption_key(&header.sender)
                {
                    let mut tag = GenericArray::<u8, <K::Key as DecryptionKey>::TagSize>::default();
                    if data.len() < tag.len() {
                        return Poll::Ready(Some(Err(InvalidMessage::Decryption.into())));
                    }
                    let (buffer, tag_bytes) = data.split_at_mut(data.len() - tag.len());
                    tag.as_mut_slice().copy_from_slice(&tag_bytes);

                    decryption_key
                        .decrypt(&[], buffer, &tag)
                        .map_err(|_| InvalidMessage::Decryption)?;
                    this.received_message_was_encrypted = true;
                }
            }

            this.received_valid_message_from = Some(sender);
            return Poll::Ready(Some(Ok(())));
        }
    }
}

#[cfg(test)]
pub mod tests {
    use std::collections::HashMap;
    use std::iter;

    use aes_gcm::{
        aead::{Aead, NewAead},
        Aes256Gcm,
    };
    use generic_array::{typenum::U12, GenericArray};
    use rand::rngs::OsRng;
    use rand::RngCore;
    use secp256k1::{PublicKey, SecretKey, SECP256K1};

    use futures::StreamExt;

    use crate::delivery::trusted_delivery::client::identity_resolver::{
        IdentityResolver, SortedIdentities,
    };
    use crate::delivery::trusted_delivery::client::insecure::crypto::{
        AesGcmDecryptionKey, DecryptionKeys, NoDecryption,
    };
    use crate::delivery::trusted_delivery::messages::{
        FixedSizeMsg, ForwardMsg, ForwardMsgHeader, ReceiveData,
    };
    use crate::Incoming;

    use super::{Incomings, InvalidMessage, ReceiveError};

    type AesKey = aes_gcm::Key<generic_array::typenum::U32>;

    fn unencrypted_message(
        sender: &SecretKey,
        recipient: PublicKey,
        body: &[u8],
    ) -> (ForwardMsgHeader, Vec<u8>) {
        let header = ForwardMsgHeader::new(sender, Some(&recipient), body);
        (header, body.to_vec())
    }
    fn encrypted_message(
        key: &AesKey,
        counter: u64,
        sender: &SecretKey,
        recipient: PublicKey,
        body: &[u8],
    ) -> (ForwardMsgHeader, Vec<u8>) {
        let aes = Aes256Gcm::new(key);

        let mut nonce = GenericArray::<u8, U12>::default();
        nonce.as_mut_slice()[..8].copy_from_slice(&counter.to_be_bytes());

        let ciphertext = aes.encrypt(&nonce, body).unwrap();
        let header = ForwardMsgHeader::new(sender, Some(&recipient), &ciphertext);
        (header, ciphertext)
    }
    fn broadcast_message(sender: &SecretKey, body: &[u8]) -> (ForwardMsgHeader, Vec<u8>) {
        let header = ForwardMsgHeader::new(sender, None, body);
        (header, body.to_vec())
    }
    fn message(sender: u16, body: &[u8]) -> Incoming<Vec<u8>> {
        Incoming {
            sender,
            msg: body.to_vec(),
        }
    }
    fn random_aes_key() -> AesKey {
        let mut key = GenericArray::default();
        OsRng.fill_bytes(key.as_mut_slice());
        key
    }
    pub fn generate_parties_sk(n: u16) -> (SortedIdentities, Vec<SecretKey>) {
        let generate_sk = || loop {
            let mut key = [0u8; 32];
            OsRng.fill_bytes(&mut key);
            if let Ok(key) = SecretKey::from_slice(&key) {
                break key;
            }
        };
        let mut keys = iter::repeat_with(generate_sk)
            .map(|sk_i| (PublicKey::from_secret_key(&SECP256K1, &sk_i), sk_i))
            .take(usize::from(n))
            .collect::<Vec<_>>();
        keys.sort_by_key(|(pk_i, _)| *pk_i);

        let (pk, sk): (Vec<_>, Vec<_>) = keys.into_iter().unzip();
        let pk = SortedIdentities::from(pk);
        (pk, sk)
    }
    struct DecryptionKeysBuilder<'pk> {
        pk: &'pk SortedIdentities,
        keys: HashMap<PublicKey, AesGcmDecryptionKey>,
    }
    impl<'pk> DecryptionKeysBuilder<'pk> {
        pub fn new(pk: &'pk SortedIdentities) -> Self {
            Self {
                pk,
                keys: HashMap::default(),
            }
        }
        pub fn add(mut self, i: u16, key: AesKey) -> Self {
            let pk = self.pk[usize::from(i)];
            let key = AesGcmDecryptionKey::new(0, Aes256Gcm::new(&key));
            self.keys.insert(pk, key);
            self
        }
        pub fn build(self) -> HashMap<PublicKey, AesGcmDecryptionKey> {
            self.keys
        }
    }
    async fn test(
        i: u16,
        parties: impl IdentityResolver + Unpin,
        decryption_keys: impl DecryptionKeys + Unpin,
        incoming_messages: &[(ForwardMsgHeader, Vec<u8>)],
        should_receive: &[Incoming<Vec<u8>>],
    ) -> Result<(), ReceiveError> {
        let mut messages = vec![];
        for (header, data) in incoming_messages {
            let len = messages.len();
            messages.resize(len + ForwardMsgHeader::SIZE + data.len(), 0);
            messages[len..len + ForwardMsgHeader::SIZE].copy_from_slice(&header.to_bytes());
            messages[len + ForwardMsgHeader::SIZE..].copy_from_slice(&data);
        }
        let pk_i = parties.lookup_party_identity(i).unwrap();
        let receive = ReceiveData::new(messages.as_slice(), ForwardMsg::new(*pk_i));
        let mut incomings = Incomings::new(parties, decryption_keys, receive);
        let mut received = vec![];
        while let Some(()) = incomings.next().await.transpose()? {
            let msg = incomings.received().unwrap();
            received.push(Incoming {
                sender: msg.sender,
                msg: msg.msg.to_vec(),
            })
        }
        assert_eq!(received, should_receive);
        Ok(())
    }

    #[tokio::test]
    async fn proceeds_an_unencrypted_message() {
        let (pk, sk) = generate_parties_sk(2);
        let input = &[unencrypted_message(&sk[0], pk[1], b"hey party 1")];
        let output = &[message(0, b"hey party 1")];
        test(1, pk, NoDecryption, input, output).await.unwrap()
    }

    #[tokio::test]
    async fn proceeds_a_broadcast_message() {
        let (pk, sk) = generate_parties_sk(2);
        let input = &[broadcast_message(&sk[0], b"hey everyone")];
        let output = &[message(0, b"hey everyone")];
        test(1, pk, NoDecryption, input, output).await.unwrap()
    }

    #[tokio::test]
    async fn proceeds_an_encrypted_message() {
        let (pk, sk) = generate_parties_sk(2);
        let aes0 = random_aes_key();
        let keys = DecryptionKeysBuilder::new(&pk).add(0, aes0).build();

        let input = &[encrypted_message(
            &aes0,
            0,
            &sk[0],
            pk[1],
            b"hey party 1, here's my secret: ...",
        )];
        let output = &[message(0, b"hey party 1, here's my secret: ...")];
        test(1, pk, keys, input, output).await.unwrap()
    }

    #[tokio::test]
    async fn ignores_messages_from_unknown_parties() {
        let (pk, sk) = generate_parties_sk(3);
        let (_outsider_pk, outsider_sk) = generate_parties_sk(1);
        let outsider_ek = random_aes_key();

        let input = &[
            unencrypted_message(&outsider_sk[0], pk[2], b"eat this yummy apple"),
            broadcast_message(&outsider_sk[0], b"who wants a free apple?"),
            broadcast_message(&sk[0], b"dont eat that forbidden fruit, children"),
            encrypted_message(&outsider_ek, 0, &outsider_sk[0], pk[2], b"he wont find out"),
        ];
        let output = &[message(0, b"dont eat that forbidden fruit, children")];
        test(2, pk, NoDecryption, input, output).await.unwrap()
    }

    #[tokio::test]
    async fn yields_error_if_message_cannot_be_decrypted() {
        let (pk, sk) = generate_parties_sk(2);
        let aes0 = random_aes_key();
        let keys = DecryptionKeysBuilder::new(&pk).add(0, aes0).build();

        let input = &[encrypted_message(
            &aes0,
            1,
            &sk[0],
            pk[1],
            b"hey party 1, here's my secret: ...",
        )];
        let output = &[message(0, b"hey party 1, here's my secret: ...")];
        let result = test(1, pk, keys, input, output).await;
        assert!(matches!(
            result,
            Err(ReceiveError::InvalidMessage(InvalidMessage::Decryption))
        ));
    }
}
