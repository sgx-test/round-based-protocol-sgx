//! # Round-based protocols execution
//!
//! Crate defines a generic round-based protocol and provides utilities for it. We give
//! formal definition below, but you may have already seen such protocols: most of [MPC] protocols
//! follow round-based communication model.
//!
//! [MPC]: https://en.wikipedia.org/wiki/Secure_multi-party_computation
//!
//! ## What is round-based protocol?
//! In round-based protocol we have `n` parties that can send messages to and receive messages
//! from other parties within rounds (number of parties `n` is known prior to starting protocol).
//!
//! At every round party may send a P2P or broadcast message, and it receives all broadcast
//! messages sent by other parties and P2P messages sent directly to it. After
//! party's received enough round messages in this round, it either proceeds (evaluates something on
//! received messages and goes to next round) or finishes the protocol.
//!
//! ## How to define own round-based protocol
//! To define own round-based protocol, you need to implement [StateMachine] trait. I.e.
//! you need to define type of [protocol message](StateMachine::MessageBody) which will be
//! transmitted on wire, determine rules how to
//! [handle incoming message](StateMachine::handle_incoming) and how to
//! [proceed state](StateMachine::proceed), etc.
//!
//! We divide methods in StateMachine on which can block and which can not. Most of MPC protocols
//! rely on computationally expensive math operations, such operations should not be executed
//! in async environment (i.e. on green-thread), that's why the only method which capable of
//! doing expensive operations is [proceed](StateMachine::proceed).
//!
//! Usually protocols assume that P2P messages are encrypted and every message is authenticated, in
//! this case underlying sink and stream must meet such requirements.
//!
//! For development purposes, you can also find useful [Simulation](crate::dev::Simulation) and
//! [AsyncSimulation](dev::AsyncSimulation) simulations that can run protocols locally.

#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod containers;

#[cfg(feature = "dev")]
#[cfg_attr(docsrs, doc(cfg(feature = "dev")))]
pub mod dev;

mod sm;
pub use sm::*;
