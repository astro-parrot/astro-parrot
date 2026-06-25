//! Autonomous [`Explorer`] implementation.
//!
//! Once started with [`OrchestratorToExplorer::StartExplorerAI`], the
//! `AiExplorer` drives itself through three phases:
//!
//! 1. **Exploring** — visit the reachable planets and record, for each, its
//!    neighbours, the basic resources it can generate, and the complex resources
//!    it can combine.
//! 2. **Collecting** — gather the basic resources its task requires.
//! 3. **Crafting** — combine them into the target complex resources, building
//!    any intermediate products first.
//!
//! It follows the `common-game` autonomous protocol: it asks the orchestrator
//! for neighbours ([`ExplorerToOrchestrator::NeighborsRequest`]) and to be moved
//! ([`ExplorerToOrchestrator::TravelToPlanetRequest`]). Every wait has a timeout
//! and stays responsive to orchestrator control messages, so a silent
//! orchestrator can never deadlock the explorer — it just stops making progress
//! and keeps answering commands.

mod ai;
mod bag;
mod knowledge;
mod recipes;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use common_game::components::resource::{ComplexResourceType, GenericResource, ResourceType};
use common_game::protocols::orchestrator_explorer::{
    ExplorerToOrchestrator, OrchestratorToExplorer, OrchestratorToExplorerKind,
};
use common_game::protocols::planet_explorer::{ExplorerToPlanet, PlanetToExplorer};
use common_game::utils::ID;
use crossbeam_channel::{Receiver, Sender, select};

use super::{BagContent, Explorer};
use bag::Bag;
use knowledge::{ExplorerKnowledge, StrategyState};

/// How long to wait for a planet's reply during an autonomous action.
const PLANET_TIMEOUT: Duration = Duration::from_millis(300);
/// How long to wait for an orchestrator reply (neighbours / move).
const ORCH_TIMEOUT: Duration = Duration::from_millis(300);
/// Consecutive no-progress turns after which a phase gives up and moves on.
const MAX_STALL: u32 = 40;

/// One message received from either of the explorer's two inbound channels.
enum Incoming {
    Orchestrator(OrchestratorToExplorer),
    Planet(PlanetToExplorer),
    OrchestratorClosed,
    PlanetClosed,
    Timeout,
}

pub struct AiExplorer {
    id: ID,
    current_planet: ID,
    rx_orchestrator: Receiver<OrchestratorToExplorer>,
    tx_orchestrator: Sender<ExplorerToOrchestrator<BagContent>>,
    tx_planet: Sender<ExplorerToPlanet>,
    rx_planet: Receiver<PlanetToExplorer>,

    bag: Bag,
    knowledge: ExplorerKnowledge,

    /// What the explorer is trying to obtain. Built adaptively after exploration
    /// when [`adaptive`](Self::adaptive) is set and no task was provided.
    task: HashMap<ResourceType, usize>,
    /// If true and the task is empty, after exploring the explorer aims to craft
    /// one of every complex resource discovered anywhere in the galaxy.
    adaptive: bool,

    state: StrategyState,
    /// How many of each complex resource have been crafted so far this run.
    produced: HashMap<ComplexResourceType, usize>,
    /// Planets a move request has failed for (avoid retrying them this run).
    unreachable: HashSet<ID>,
    /// Resources no known planet can provide (avoid chasing them forever).
    unobtainable: HashSet<ResourceType>,
    /// Consecutive no-progress steps in the current phase.
    stall: u32,

    /// Whether the autonomous AI is active (toggled by Start/Stop).
    running: bool,
    /// Whether the explorer thread should keep living (cleared by Kill).
    alive: bool,
}

impl Explorer for AiExplorer {
    fn new(
        id: ID,
        current_planet: ID,
        rx_orchestrator: Receiver<OrchestratorToExplorer>,
        tx_orchestrator: Sender<ExplorerToOrchestrator<BagContent>>,
        tx_current_planet: Sender<ExplorerToPlanet>,
        rx_planet: Receiver<PlanetToExplorer>,
    ) -> Self {
        Self {
            id,
            current_planet,
            rx_orchestrator,
            tx_orchestrator,
            tx_planet: tx_current_planet,
            rx_planet,
            bag: Bag::default(),
            knowledge: ExplorerKnowledge::default(),
            task: HashMap::new(),
            adaptive: true,
            state: StrategyState::Exploring,
            produced: HashMap::new(),
            unreachable: HashSet::new(),
            unobtainable: HashSet::new(),
            stall: 0,
            running: false,
            alive: true,
        }
    }

    fn run(&mut self) -> Result<(), String> {
        self.knowledge.entry(self.current_planet);
        // Turn-based driving: the orchestrator gives the explorer a turn by
        // sending a `BagContentRequest`. On its turn the explorer performs one
        // autonomous step — which may query/move through the orchestrator and
        // generate/craft directly with the planet — and ends the turn by
        // reporting its bag. Every other message is handled as it arrives.
        while self.alive {
            match self.rx_orchestrator.recv() {
                Ok(OrchestratorToExplorer::BagContentRequest) => {
                    if self.running {
                        self.advance();
                    }
                    self.reply(ExplorerToOrchestrator::BagContentResponse {
                        explorer_id: self.id,
                        bag_content: self.bag.to_content(),
                    });
                }
                Ok(msg) => self.handle_orchestrator(msg),
                Err(_) => break, // orchestrator gone
            }
        }
        Ok(())
    }
}

impl AiExplorer {
    // ------------------------------------------------------------------
    // Orchestrator message handling
    // ------------------------------------------------------------------

    /// Handles a single orchestrator message. Control messages mutate the
    /// explorer's lifecycle; "manual mode" requests round-trip to the planet so
    /// the GUI keeps working even while the AI is active.
    fn handle_orchestrator(&mut self, msg: OrchestratorToExplorer) {
        match msg {
            OrchestratorToExplorer::StartExplorerAI => {
                self.running = true;
                self.stall = 0;
                self.reply(ExplorerToOrchestrator::StartExplorerAIResult { explorer_id: self.id });
            }
            OrchestratorToExplorer::StopExplorerAI => {
                self.running = false;
                self.reply(ExplorerToOrchestrator::StopExplorerAIResult { explorer_id: self.id });
            }
            OrchestratorToExplorer::ResetExplorerAI => {
                self.knowledge = ExplorerKnowledge::default();
                self.bag.clear();
                self.produced.clear();
                self.unreachable.clear();
                self.unobtainable.clear();
                self.task.clear();
                self.state = StrategyState::Exploring;
                self.stall = 0;
                self.knowledge.entry(self.current_planet);
                self.reply(ExplorerToOrchestrator::ResetExplorerAIResult { explorer_id: self.id });
            }
            OrchestratorToExplorer::KillExplorer => {
                self.alive = false;
                self.reply(ExplorerToOrchestrator::KillExplorerResult { explorer_id: self.id });
            }
            OrchestratorToExplorer::CurrentPlanetRequest => {
                self.reply(ExplorerToOrchestrator::CurrentPlanetResult {
                    explorer_id: self.id,
                    planet_id: self.current_planet,
                });
            }
            OrchestratorToExplorer::BagContentRequest => {
                self.reply(ExplorerToOrchestrator::BagContentResponse {
                    explorer_id: self.id,
                    bag_content: self.bag.to_content(),
                });
            }
            OrchestratorToExplorer::MoveToPlanet { sender_to_new_planet, planet_id } => {
                // A move pushed by the orchestrator (manual mode or relocation).
                if let Some(tx) = sender_to_new_planet {
                    self.tx_planet = tx;
                    self.current_planet = planet_id;
                    self.knowledge.entry(planet_id);
                }
                self.reply(ExplorerToOrchestrator::MovedToPlanetResult {
                    explorer_id: self.id,
                    planet_id: self.current_planet,
                });
            }
            OrchestratorToExplorer::NeighborsResponse { neighbors } => {
                // Unsolicited neighbour info (the solicited path is in `ai.rs`).
                self.knowledge.entry(self.current_planet).neighbors = neighbors.into_iter().collect();
            }
            OrchestratorToExplorer::SupportedResourceRequest => {
                let supported = match self
                    .planet_roundtrip(ExplorerToPlanet::SupportedResourceRequest { explorer_id: self.id })
                {
                    Some(PlanetToExplorer::SupportedResourceResponse { resource_list }) => {
                        self.knowledge.entry(self.current_planet).basics = resource_list.clone();
                        resource_list
                    }
                    _ => Default::default(),
                };
                self.reply(ExplorerToOrchestrator::SupportedResourceResult {
                    explorer_id: self.id,
                    supported_resources: supported,
                });
            }
            OrchestratorToExplorer::SupportedCombinationRequest => {
                let supported = match self
                    .planet_roundtrip(ExplorerToPlanet::SupportedCombinationRequest { explorer_id: self.id })
                {
                    Some(PlanetToExplorer::SupportedCombinationResponse { combination_list }) => {
                        self.knowledge.entry(self.current_planet).combinations = combination_list.clone();
                        combination_list
                    }
                    _ => Default::default(),
                };
                self.reply(ExplorerToOrchestrator::SupportedCombinationResult {
                    explorer_id: self.id,
                    combination_list: supported,
                });
            }
            OrchestratorToExplorer::GenerateResourceRequest { to_generate } => {
                let generated = match self.planet_roundtrip(ExplorerToPlanet::GenerateResourceRequest {
                    explorer_id: self.id,
                    resource: to_generate,
                }) {
                    Some(PlanetToExplorer::GenerateResourceResponse { resource: Some(r) }) => {
                        self.bag.add_basic(r);
                        Ok(())
                    }
                    Some(PlanetToExplorer::GenerateResourceResponse { resource: None }) => {
                        Err("planet could not generate the resource".to_string())
                    }
                    _ => Err("no response from planet".to_string()),
                };
                self.reply(ExplorerToOrchestrator::GenerateResourceResponse {
                    explorer_id: self.id,
                    generated,
                });
            }
            OrchestratorToExplorer::CombineResourceRequest { to_generate } => {
                let generated = self.manual_combine(to_generate);
                self.reply(ExplorerToOrchestrator::CombineResourceResponse {
                    explorer_id: self.id,
                    generated,
                });
            }
        }
    }

    /// Manual-mode crafting: build the request from the bag, round-trip to the
    /// planet, and recover ingredients on failure.
    fn manual_combine(&mut self, ty: ComplexResourceType) -> Result<(), String> {
        let Some(req) = recipes::build_request(&mut self.bag, ty) else {
            return Err(format!("missing ingredients to craft {ty:?}"));
        };
        match self.planet_roundtrip(ExplorerToPlanet::CombineResourceRequest { explorer_id: self.id, msg: req }) {
            Some(PlanetToExplorer::CombineResourceResponse { complex_response: Ok(c) }) => {
                self.bag.add_complex(c);
                Ok(())
            }
            Some(PlanetToExplorer::CombineResourceResponse { complex_response: Err((e, g1, g2)) }) => {
                self.recover(g1);
                self.recover(g2);
                Err(e)
            }
            _ => Err("no response from planet".to_string()),
        }
    }

    // ------------------------------------------------------------------
    // Low-level communication
    // ------------------------------------------------------------------

    /// Drains every orchestrator message currently queued, without blocking.
    fn drain_orchestrator(&mut self) {
        while let Ok(msg) = self.rx_orchestrator.try_recv() {
            self.handle_orchestrator(msg);
            if !self.alive {
                return;
            }
        }
    }

    /// Receives from either inbound channel, or returns [`Incoming::Timeout`].
    fn recv_any(&self, timeout: Duration) -> Incoming {
        select! {
            recv(self.rx_orchestrator) -> m => match m {
                Ok(msg) => Incoming::Orchestrator(msg),
                Err(_) => Incoming::OrchestratorClosed,
            },
            recv(self.rx_planet) -> m => match m {
                Ok(msg) => Incoming::Planet(msg),
                Err(_) => Incoming::PlanetClosed,
            },
            default(timeout) => Incoming::Timeout,
        }
    }

    /// Waits for the next planet reply, staying responsive to the orchestrator.
    /// Returns `None` on timeout or if a channel closes.
    fn await_planet(&mut self, timeout: Duration) -> Option<PlanetToExplorer> {
        let deadline = Instant::now() + timeout;
        loop {
            if !self.alive {
                return None;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            match self.recv_any(remaining) {
                // A stopped planet only ever answers with `Stopped`: the planet
                // we are talking to has been destroyed, so remember that and
                // treat the request as unanswered.
                Incoming::Planet(PlanetToExplorer::Stopped) => {
                    self.knowledge.mark_dead(self.current_planet);
                    return None;
                }
                Incoming::Planet(msg) => return Some(msg),
                Incoming::Orchestrator(msg) => self.handle_orchestrator(msg),
                Incoming::OrchestratorClosed => {
                    self.alive = false;
                    return None;
                }
                Incoming::PlanetClosed | Incoming::Timeout => return None,
            }
        }
    }

    /// Waits for a specific orchestrator reply, handling any other orchestrator
    /// message in the meantime. Returns `None` on timeout.
    fn await_orchestrator(
        &mut self,
        kind: OrchestratorToExplorerKind,
        timeout: Duration,
    ) -> Option<OrchestratorToExplorer> {
        let deadline = Instant::now() + timeout;
        loop {
            if !self.alive {
                return None;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            match self.recv_any(remaining) {
                Incoming::Orchestrator(msg) => {
                    if OrchestratorToExplorerKind::from(&msg) == kind {
                        return Some(msg);
                    }
                    self.handle_orchestrator(msg);
                }
                Incoming::Planet(_) => {} // stray planet reply: drop it
                Incoming::OrchestratorClosed => {
                    self.alive = false;
                    return None;
                }
                Incoming::PlanetClosed | Incoming::Timeout => return None,
            }
        }
    }

    /// Blocking round-trip to the current planet (used by manual-mode handlers).
    fn planet_roundtrip(&self, msg: ExplorerToPlanet) -> Option<PlanetToExplorer> {
        self.tx_planet.send(msg).ok()?;
        self.rx_planet.recv_timeout(PLANET_TIMEOUT).ok()
    }

    fn reply(&self, msg: ExplorerToOrchestrator<BagContent>) {
        let _ = self.tx_orchestrator.send(msg);
    }

    fn send_orchestrator(&self, msg: ExplorerToOrchestrator<BagContent>) {
        let _ = self.tx_orchestrator.send(msg);
    }

    fn send_planet(&self, msg: ExplorerToPlanet) {
        let _ = self.tx_planet.send(msg);
    }

    /// Puts a recovered ingredient back into the bag after a failed craft.
    fn recover(&mut self, resource: GenericResource) {
        match resource {
            GenericResource::BasicResources(b) => self.bag.add_basic(b),
            GenericResource::ComplexResources(c) => self.bag.add_complex(c),
        }
    }
}
