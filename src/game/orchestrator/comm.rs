//! Communication helpers for the orchestrator.
//!
//! Planets and explorers each share one return channel back to the orchestrator.
//! A [`Demux`] splits that single stream per sender id (buffering messages that
//! arrive out of order), and a [`CommCenter`] adds the request/acknowledge
//! pattern used throughout the orchestrator: send a message to an actor and
//! block (with a timeout) until that actor answers with the expected kind.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use astro_parrot::BagContent;
use common_game::protocols::orchestrator_explorer::{
    ExplorerToOrchestrator, ExplorerToOrchestratorKind, OrchestratorToExplorer,
};
use common_game::protocols::orchestrator_planet::{
    OrchestratorToPlanet, PlanetToOrchestrator, PlanetToOrchestratorKind,
};
use common_game::utils::ID;
use crossbeam_channel::{Receiver, Sender};

const RECV_TIMEOUT: Duration = Duration::from_millis(500);

/// A message that carries the id of its sender and a comparable "kind" tag.
pub trait Tagged {
    type Kind: PartialEq + Copy + std::fmt::Debug;
    fn sender_id(&self) -> ID;
    fn kind(&self) -> Self::Kind;
}

impl Tagged for PlanetToOrchestrator {
    type Kind = PlanetToOrchestratorKind;
    fn sender_id(&self) -> ID {
        self.planet_id()
    }
    fn kind(&self) -> Self::Kind {
        self.into()
    }
}

impl Tagged for ExplorerToOrchestrator<BagContent> {
    type Kind = ExplorerToOrchestratorKind;
    fn sender_id(&self) -> ID {
        self.explorer_id()
    }
    fn kind(&self) -> Self::Kind {
        self.into()
    }
}

/// Splits a shared receiver into per-sender streams.
struct Demux<M: Tagged> {
    rx: Receiver<M>,
    buffers: HashMap<ID, VecDeque<M>>,
}

impl<M: Tagged> Demux<M> {
    fn new(rx: Receiver<M>) -> Self {
        Self { rx, buffers: HashMap::new() }
    }

    fn recv_from(&mut self, id: ID) -> Result<M, String> {
        if let Some(buf) = self.buffers.get_mut(&id)
            && let Some(msg) = buf.pop_front()
        {
            return Ok(msg);
        }

        let start = Instant::now();
        while start.elapsed() < RECV_TIMEOUT {
            match self.rx.recv_timeout(RECV_TIMEOUT) {
                Ok(msg) => {
                    if msg.sender_id() == id {
                        return Ok(msg);
                    }
                    self.buffers.entry(msg.sender_id()).or_default().push_back(msg);
                }
                Err(e) => return Err(format!("error waiting for message from {id}: {e}")),
            }
        }
        Err(format!("timeout waiting for message from {id}"))
    }
}

/// Request/acknowledge communication center over a set of per-id senders and one
/// demultiplexed return channel.
pub struct CommCenter<S, M: Tagged> {
    tx: HashMap<ID, Sender<S>>,
    demux: Demux<M>,
}

impl<S, M: Tagged> CommCenter<S, M> {
    pub fn new(rx: Receiver<M>) -> Self {
        Self { tx: HashMap::new(), demux: Demux::new(rx) }
    }

    pub fn register(&mut self, id: ID, tx: Sender<S>) {
        self.tx.insert(id, tx);
    }

    pub fn remove(&mut self, id: ID) {
        self.tx.remove(&id);
    }

    pub fn send_to(&self, id: ID, msg: S) -> Result<(), String> {
        self.tx
            .get(&id)
            .ok_or_else(|| format!("no channel registered for {id}"))?
            .send(msg)
            .map_err(|_| format!("failed to send to {id}"))
    }

    /// Waits for the next message from actor `id`, of any kind.
    pub fn recv_from(&mut self, id: ID) -> Result<M, String> {
        self.demux.recv_from(id)
    }

    /// Sends `msg` to actor `id` and waits for a reply of kind `expected`.
    pub fn req_ack(&mut self, id: ID, msg: S, expected: M::Kind) -> Result<M, String> {
        self.send_to(id, msg)?;
        let res = self.demux.recv_from(id)?;
        if res.kind() == expected {
            Ok(res)
        } else {
            Err(format!("expected {expected:?} from {id}, got {:?}", res.kind()))
        }
    }
}

pub type PlanetComm = CommCenter<OrchestratorToPlanet, PlanetToOrchestrator>;
pub type ExplorerComm = CommCenter<OrchestratorToExplorer, ExplorerToOrchestrator<BagContent>>;
