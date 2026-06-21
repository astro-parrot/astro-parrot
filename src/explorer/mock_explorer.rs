//! Default [`Explorer`] implementation.

use std::time::Duration;

use common_game::components::resource::{
    BasicResource, ComplexResourceRequest, ComplexResourceType, ResourceType,
};
use common_game::protocols::orchestrator_explorer::{ExplorerToOrchestrator, OrchestratorToExplorer};
use common_game::protocols::planet_explorer::{ExplorerToPlanet, PlanetToExplorer};
use common_game::utils::ID;
use crossbeam_channel::{Receiver, Sender};

use super::{BagContent, Explorer};

const PLANET_TIMEOUT: Duration = Duration::from_millis(500);

/// A straightforward explorer: it forwards the orchestrator's commands to its
/// current planet, keeps the obtained resources in a bag, and can craft a
/// Diamond out of two Carbon resources.
pub struct MockExplorer {
    id: ID,
    current_planet: ID,
    rx_orchestrator: Receiver<OrchestratorToExplorer>,
    tx_orchestrator: Sender<ExplorerToOrchestrator<BagContent>>,
    tx_planet: Sender<ExplorerToPlanet>,
    rx_planet: Receiver<PlanetToExplorer>,
    basics: Vec<BasicResource>,
}

impl Explorer for MockExplorer {
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
        }
    }

    fn run(&mut self) -> Result<(), String> {
        loop {
            match self.rx_orchestrator.recv() {
                Ok(OrchestratorToExplorer::KillExplorer) => {
                    self.reply(ExplorerToOrchestrator::KillExplorerResult { explorer_id: self.id });
                    return Ok(());
                }
                Ok(msg) => self.handle(msg),
                Err(_) => return Ok(()), // orchestrator gone
            }
        }
    }
}

impl MockExplorer {
    fn handle(&mut self, msg: OrchestratorToExplorer) {
        match msg {
            OrchestratorToExplorer::StartExplorerAI => {
                self.reply(ExplorerToOrchestrator::StartExplorerAIResult { explorer_id: self.id });
            }
            OrchestratorToExplorer::StopExplorerAI => {
                self.reply(ExplorerToOrchestrator::StopExplorerAIResult { explorer_id: self.id });
            }
            OrchestratorToExplorer::ResetExplorerAI => {
                self.basics.clear();
                self.reply(ExplorerToOrchestrator::ResetExplorerAIResult { explorer_id: self.id });
            }
            OrchestratorToExplorer::CurrentPlanetRequest => {
                self.reply(ExplorerToOrchestrator::CurrentPlanetResult {
                    explorer_id: self.id,
                    planet_id: self.current_planet,
                });
            }
            OrchestratorToExplorer::MoveToPlanet {
                sender_to_new_planet,
                planet_id,
            } => {
                if let Some(tx) = sender_to_new_planet {
                    self.tx_planet = tx;
                    self.current_planet = planet_id;
                }
                self.reply(ExplorerToOrchestrator::MovedToPlanetResult {
                    explorer_id: self.id,
                    planet_id: self.current_planet,
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
            OrchestratorToExplorer::BagContentRequest => {
                let bag_content = self.bag();
                self.reply(ExplorerToOrchestrator::BagContentResponse {
                    explorer_id: self.id,
                    bag_content,
                });
            }
            // Autonomous-mode hint we don't act on here; KillExplorer is handled in `run`.
            OrchestratorToExplorer::NeighborsResponse { .. }
            | OrchestratorToExplorer::KillExplorer => {}
        }
    }

    fn generate_basic(
        &mut self,
        resource: common_game::components::resource::BasicResourceType,
    ) -> Result<(), String> {
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
                    Some(PlanetToExplorer::CombineResourceResponse {
                        complex_response: Ok(_),
                    }) => Ok(()),
                    Some(PlanetToExplorer::CombineResourceResponse {
                        complex_response: Err((e, g1, g2)),
                    }) => {
                        // Recover the ingredients the planet handed back.
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
            *bag
                .content
                .entry(ResourceType::Basic(r.get_type()))
                .or_default() += 1;
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
