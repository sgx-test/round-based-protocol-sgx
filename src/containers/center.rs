#![allow(dead_code)]
use std::ops;

use crate::sm::Msg;
use std::vec::Vec;

use super::store_err::StoreErr;
use super::traits::{MessageContainer, MessageStore};

/// Received center messages from center
#[derive(Debug, Clone)]
pub struct CenterMsgs<B> {
    msgs: Vec<B>,
}

impl<B> CenterMsgs<B>
where
    B: 'static + Clone,
{
    /// Turns a container into iterator of messages with parties indexes (1 <= i <= n)
    pub fn into_iter_indexed(self) -> impl Iterator<Item = (u16, B)> {
        self.msgs
            .into_iter()
            .enumerate()
            .map(move |(i, m)| (i as u16, m))
    }

    /// Turns container into vec of `n-1` messages
    pub fn into_vec(self) -> Vec<B> {
        self.msgs
    }
}

impl<B> ops::Index<u16> for CenterMsgs<B> {
    type Output = B;

    /// Takes party index i and returns received message (1 <= i <= n)
    fn index(&self, index: u16) -> &Self::Output {
        &self.msgs[usize::from(index)]
    }
}

impl<B> IntoIterator for CenterMsgs<B> {
    type Item = B;
    type IntoIter = <Vec<B> as IntoIterator>::IntoIter;

    /// Returns messages in ascending party's index order
    fn into_iter(self) -> Self::IntoIter {
        self.msgs.into_iter()
    }
}

impl<M> MessageContainer for CenterMsgs<M> {
    type Store = CenterMsgsStore<M>;
}

/// Receives broadcast messages from every protocol participant
#[derive(Clone)]
pub struct CenterMsgsStore<M> {
    msgs: Vec<Option<M>>,
    msgs_left: usize,
}

impl<M: Clone> CenterMsgsStore<M> {
    /// Constructs store. Takes this party index and total number of parties.
    pub fn new(parties_n: u16) -> Self {
        let parties_n = usize::from(parties_n);
        Self {
            msgs: std::iter::repeat_with(|| None).take(parties_n).collect(),
            msgs_left: parties_n,
        }
    }

    /// Amount of received messages so far
    pub fn messages_received(&self) -> usize {
        self.msgs.len() - self.msgs_left
    }
    /// Total amount of wanted messages (n)
    pub fn messages_total(&self) -> usize {
        self.msgs.len()
    }
}

impl<M> MessageStore for CenterMsgsStore<M> {
    type M = M;
    type Err = StoreErr;
    type Output = CenterMsgs<M>;

    fn push_msg(&mut self, msg: Msg<Self::M>) -> Result<(), Self::Err> {
        if msg.sender == 0 {
            return Err(StoreErr::UnknownSender { sender: msg.sender });
        }
        if msg.receiver.is_some() {
            return Err(StoreErr::ExpectedCenter);
        }
        let party_j = usize::from(msg.sender);
        let slot = self
            .msgs
            .get_mut(party_j - 1)
            .ok_or(StoreErr::UnknownSender { sender: msg.sender })?;
        if slot.is_some() {
            return Ok(());
        }
        *slot = Some(msg.body);
        self.msgs_left -= 1;

        Ok(())
    }

    fn contains_msg_from(&self, sender: u16) -> bool {
        let party_j = usize::from(sender);
        match self.msgs.get(party_j - 1) {
            None => false,
            Some(None) => false,
            Some(Some(_)) => true,
        }
    }

    fn wants_more(&self) -> bool {
        self.msgs_left > 0
    }

    fn finish(self) -> Result<Self::Output, Self::Err> {
        if self.msgs_left > 0 {
            return Err(StoreErr::WantsMoreMessages);
        }
        Ok(CenterMsgs {
            msgs: self.msgs.into_iter().map(Option::unwrap).collect(),
        })
    }

    fn blame(&self) -> (u16, Vec<u16>) {
        let guilty_parties = self
            .msgs
            .iter()
            .enumerate()
            .flat_map(|(i, m)| if m.is_none() { Some(i as u16) } else { None })
            .collect();
        (self.msgs_left as u16, guilty_parties)
    }
}
