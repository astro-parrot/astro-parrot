//! Autonomous [`Explorer`] implementation.
//!
//! The AI explorer mines Carbon and crafts Diamonds on its current planet
//! without any external command. It starts acting as soon as it receives
//! [`OrchestratorToExplorer::StartExplorerAI`] and stays responsive to
//! every orchestrator message in between steps.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use common_game::components::resource::{
    BasicResource, BasicResourceType, ComplexResourceRequest, ComplexResourceType, ResourceType,
};
use common_game::protocols::orchestrator_explorer::{ExplorerToOrchestrator, OrchestratorToExplorer};
use common_game::protocols::planet_explorer::{ExplorerToPlanet, PlanetToExplorer};
use common_game::utils::ID;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use super::{BagContent, Explorer};

/// How long to wait for a planet response during an autonomous step.
const PLANET_TIMEOUT: Duration = Duration::from_millis(200);
/// How often the main loop polls for orchestrator messages when idle.
const POLL_INTERVAL: Duration = Duration::from_millis(10);
/// Minimum time between autonomous mining/crafting steps.
const STEP_INTERVAL: Duration = Duration::from_millis(1500);

pub struct AiExplorer {
    id: ID,
    current_planet: ID,
    rx_orchestrator: Receiver<OrchestratorToExplorer>,
    tx_orchestrator: Sender<ExplorerToOrchestrator<BagContent>>,
    tx_planet: Sender<ExplorerToPlanet>,
    rx_planet: Receiver<PlanetToExplorer>,
    /// Basic resources in the bag.
    basics: Vec<BasicResource>,
    /// Count of crafted complex resources (tracked separately).
    complex: HashMap<ComplexResourceType, usize>,
    ai_running: bool,
    last_step: Instant,
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
            basics: Vec::new(),
            complex: HashMap::new(),
            ai_running: false,
            last_step: Instant::now(),
        }
    }

    fn run(&mut self) -> Result<(), String> {
        loop {
            match self.rx_orchestrator.recv_timeout(POLL_INTERVAL) {
                Ok(OrchestratorToExplorer::KillExplorer) => {
                    self.reply(ExplorerToOrchestrator::KillExplorerResult { explorer_id: self.id });
                    return Ok(());
                }
                Ok(msg) => self.handle(msg),
                Err(RecvTimeoutError::Timeout) => {
                    if self.ai_running && self.last_step.elapsed() >= STEP_INTERVAL {
                        self.autonomous_step();
                        self.last_step = Instant::now();
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
        }
    }
}

impl AiExplorer {
    fn handle(&mut self, msg: OrchestratorToExplorer) {
        match msg {
            OrchestratorToExplorer::StartExplorerAI => {
                self.ai_running = true;
                self.reply(ExplorerToOrchestrator::StartExplorerAIResult { explorer_id: self.id });
            }
            OrchestratorToExplorer::StopExplorerAI => {
                self.ai_running = false;
                self.reply(ExplorerToOrchestrator::StopExplorerAIResult { explorer_id: self.id });
            }
            OrchestratorToExplorer::ResetExplorerAI => {
                self.basics.clear();
                self.complex.clear();
                self.reply(ExplorerToOrchestrator::ResetExplorerAIResult { explorer_id: self.id });
            }
            OrchestratorToExplorer::CurrentPlanetRequest => {
                self.reply(ExplorerToOrchestrator::CurrentPlanetResult {
                    explorer_id: self.id,
                    planet_id: self.current_planet,
                });
            }
            OrchestratorToExplorer::MoveToPlanet { sender_to_new_planet, planet_id } => {
                if let Some(tx) = sender_to_new_planet {
                    self.tx_planet = tx;
                }
                self.current_planet = planet_id;
                self.reply(ExplorerToOrchestrator::MovedToPlanetResult {
                    explorer_id: self.id,
                    planet_id: self.current_planet,
                });
            }
            OrchestratorToExplorer::BagContentRequest => {
                self.reply(ExplorerToOrchestrator::BagContentResponse {
                    explorer_id: self.id,
                    bag_content: self.bag(),
                });
            }
            OrchestratorToExplorer::SupportedResourceRequest => {
                let supported = match self.planet_roundtrip(ExplorerToPlanet::SupportedResourceRequest {
                    explorer_id: self.id,
                }) {
                    Some(PlanetToExplorer::SupportedResourceResponse { resource_list }) => resource_list,
                    _ => Default::default(),
                };
                self.reply(ExplorerToOrchestrator::SupportedResourceResult {
                    explorer_id: self.id,
                    supported_resources: supported,
                });
            }
            OrchestratorToExplorer::SupportedCombinationRequest => {
                let supported = match self.planet_roundtrip(ExplorerToPlanet::SupportedCombinationRequest {
                    explorer_id: self.id,
                }) {
                    Some(PlanetToExplorer::SupportedCombinationResponse { combination_list }) => combination_list,
                    _ => Default::default(),
                };
                self.reply(ExplorerToOrchestrator::SupportedCombinationResult {
                    explorer_id: self.id,
                    combination_list: supported,
                });
            }
            // Manual commands still work (backward compatible with GUI).
            OrchestratorToExplorer::GenerateResourceRequest { to_generate } => {
                let generated = self.generate_basic(to_generate);
                self.reply(ExplorerToOrchestrator::GenerateResourceResponse {
                    explorer_id: self.id,
                    generated,
                });
            }
            OrchestratorToExplorer::CombineResourceRequest { to_generate } => {
                let generated = self.craft_complex(to_generate);
                self.reply(ExplorerToOrchestrator::CombineResourceResponse {
                    explorer_id: self.id,
                    generated,
                });
            }
            OrchestratorToExplorer::NeighborsResponse { .. } | OrchestratorToExplorer::KillExplorer => {}
        }
    }

    // -----------------------------------------------------------------------
    // Autonomous behaviour
    // -----------------------------------------------------------------------

    /// One AI step: mine Carbon until we have 2, then craft a Diamond.
    fn autonomous_step(&mut self) {
        let carbon_count = self
            .basics
            .iter()
            .filter(|r| matches!(r, BasicResource::Carbon(_)))
            .count();

        if carbon_count < 2 {
            self.auto_mine_carbon();
        } else {
            self.auto_craft_diamond();
        }
    }

    fn auto_mine_carbon(&mut self) {
        let _ = self.tx_planet.send(ExplorerToPlanet::GenerateResourceRequest {
            explorer_id: self.id,
            resource: BasicResourceType::Carbon,
        });
        if let Ok(PlanetToExplorer::GenerateResourceResponse { resource: Some(r) }) =
            self.rx_planet.recv_timeout(PLANET_TIMEOUT)
        {
            self.basics.push(r);
        }
    }

    fn auto_craft_diamond(&mut self) {
        let Some((c1, c2)) = self.take_two_carbon() else { return };
        let _ = self.tx_planet.send(ExplorerToPlanet::CombineResourceRequest {
            explorer_id: self.id,
            msg: ComplexResourceRequest::Diamond(c1, c2),
        });
        match self.rx_planet.recv_timeout(PLANET_TIMEOUT) {
            Ok(PlanetToExplorer::CombineResourceResponse { complex_response: Ok(_) }) => {
                *self.complex.entry(ComplexResourceType::Diamond).or_default() += 1;
            }
            Ok(PlanetToExplorer::CombineResourceResponse { complex_response: Err((_, g1, g2)) }) => {
                // Planet refused — put the ingredients back.
                if let Ok(c) = g1.to_carbon() {
                    self.basics.push(c.to_basic());
                }
                if let Ok(c) = g2.to_carbon() {
                    self.basics.push(c.to_basic());
                }
            }
            _ => {
                // Timeout or unexpected message — ingredients are lost for this step.
                // The carbon was already removed from basics; reset timer so we try again soon.
                self.last_step = Instant::now() - STEP_INTERVAL + Duration::from_millis(200);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Manual-command helpers (unchanged from MockExplorer)
    // -----------------------------------------------------------------------

    fn generate_basic(&mut self, resource: BasicResourceType) -> Result<(), String> {
        match self.planet_roundtrip(ExplorerToPlanet::GenerateResourceRequest {
            explorer_id: self.id,
            resource,
        }) {
            Some(PlanetToExplorer::GenerateResourceResponse { resource: Some(r) }) => {
                self.basics.push(r);
                Ok(())
            }
            Some(PlanetToExplorer::GenerateResourceResponse { resource: None }) => {
                Err("planet could not generate the resource".to_string())
            }
            _ => Err("no response from planet".to_string()),
        }
    }

    fn craft_complex(&mut self, ty: ComplexResourceType) -> Result<(), String> {
        match ty {
            ComplexResourceType::Diamond => {
                let (c1, c2) = self
                    .take_two_carbon()
                    .ok_or_else(|| "need 2 Carbon to craft a Diamond".to_string())?;
                match self.planet_roundtrip(ExplorerToPlanet::CombineResourceRequest {
                    explorer_id: self.id,
                    msg: ComplexResourceRequest::Diamond(c1, c2),
                }) {
                    Some(PlanetToExplorer::CombineResourceResponse { complex_response: Ok(_) }) => {
                        *self.complex.entry(ComplexResourceType::Diamond).or_default() += 1;
                        Ok(())
                    }
                    Some(PlanetToExplorer::CombineResourceResponse {
                        complex_response: Err((e, g1, g2)),
                    }) => {
                        if let Ok(c) = g1.to_carbon() {
                            self.basics.push(c.to_basic());
                        }
                        if let Ok(c) = g2.to_carbon() {
                            self.basics.push(c.to_basic());
                        }
                        Err(e)
                    }
                    _ => Err("no response from planet".to_string()),
                }
            }
            other => Err(format!("explorer cannot craft {other:?}")),
        }
    }

    fn take_two_carbon(
        &mut self,
    ) -> Option<(
        common_game::components::resource::Carbon,
        common_game::components::resource::Carbon,
    )> {
        let idxs: Vec<usize> = self
            .basics
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(r, BasicResource::Carbon(_)))
            .map(|(i, _)| i)
            .take(2)
            .collect();
        if idxs.len() < 2 {
            return None;
        }
        let second = self.basics.remove(idxs[1]);
        let first = self.basics.remove(idxs[0]);
        Some((first.to_carbon().ok()?, second.to_carbon().ok()?))
    }

    fn bag(&self) -> BagContent {
        let mut bag = BagContent::default();
        for r in &self.basics {
            *bag.content.entry(ResourceType::Basic(r.get_type())).or_default() += 1;
        }
        for (&ty, &count) in &self.complex {
            if count > 0 {
                *bag.content.entry(ResourceType::Complex(ty)).or_default() += count;
            }
        }
        bag
    }

    fn planet_roundtrip(&self, msg: ExplorerToPlanet) -> Option<PlanetToExplorer> {
        self.tx_planet.send(msg).ok()?;
        self.rx_planet.recv_timeout(PLANET_TIMEOUT).ok()
    }

    fn reply(&self, msg: ExplorerToOrchestrator<BagContent>) {
        let _ = self.tx_orchestrator.send(msg);
    }
}
