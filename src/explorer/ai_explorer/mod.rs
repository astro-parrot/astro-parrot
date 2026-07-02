// Autonomous explorer. Once started it drives itself: explore the galaxy,
// collect the basics its goal needs, craft the target complex resources. It
// talks to the orchestrator (neighbours / travel) and the planets (query /
// generate / combine) over the common-game protocol. Every wait has a timeout,
// so a silent orchestrator or planet slows it down but never deadlocks it.

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

const PLANET_TIMEOUT: Duration = Duration::from_millis(300);
const ORCH_TIMEOUT: Duration = Duration::from_millis(300);
// Give up on a phase after this many turns without progress.
const MAX_STALL: u32 = 40;

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

    task: HashMap<ResourceType, usize>,
    // When set and no task was given, pick a goal after exploring.
    adaptive: bool,

    state: StrategyState,
    // Complex resources crafted so far this run.
    produced: HashMap<ComplexResourceType, usize>,
    // Planets we failed to move to; resources no planet can provide.
    unreachable: HashSet<ID>,
    unobtainable: HashSet<ResourceType>,
    // Planets currently out of energy; try elsewhere before coming back.
    depleted: HashSet<ID>,
    stall: u32,

    running: bool, // AI active (Start/Stop)
    alive: bool,   // thread should live (cleared by Kill)
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
            depleted: HashSet::new(),
            stall: 0,
            running: false,
            alive: true,
        }
    }

    fn run(&mut self) -> Result<(), String> {
        self.knowledge.entry(self.current_planet);
        // The orchestrator polls each explorer with a BagContentRequest, which is
        // that explorer's turn: do one step of work, then report the bag. Other
        // messages are handled as they arrive.
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
                Err(_) => break,
            }
        }
        Ok(())
    }
}

impl AiExplorer {
    // Control messages drive the lifecycle; the manual-mode requests round-trip
    // to the planet so the GUI still works while the AI is running.
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
                self.depleted.clear();
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
                // Move pushed by the orchestrator (manual mode or relocation).
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

    fn drain_orchestrator(&mut self) {
        while let Ok(msg) = self.rx_orchestrator.try_recv() {
            self.handle_orchestrator(msg);
            if !self.alive {
                return;
            }
        }
    }

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

    // Wait for a planet reply, but keep answering the orchestrator meanwhile.
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
                // A stopped planet only answers `Stopped`: it's been destroyed.
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

    // Wait for a specific orchestrator reply, handling anything else in between.
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
                Incoming::Planet(_) => {}
                Incoming::OrchestratorClosed => {
                    self.alive = false;
                    return None;
                }
                Incoming::PlanetClosed | Incoming::Timeout => return None,
            }
        }
    }

    // Blocking round-trip used by the manual-mode handlers.
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

    // Put an ingredient back after a failed craft.
    fn recover(&mut self, resource: GenericResource) {
        match resource {
            GenericResource::BasicResources(b) => self.bag.add_basic(b),
            GenericResource::ComplexResources(c) => self.bag.add_complex(c),
        }
    }
}
