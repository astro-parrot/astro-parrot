//! Builds the explorer implementations written by the group's members.

use astro_parrot::{AiExplorer, BagContent, Explorer, MockExplorer, SmartExplorer};
use common_game::protocols::orchestrator_explorer::{ExplorerToOrchestrator, OrchestratorToExplorer};
use common_game::protocols::planet_explorer::{ExplorerToPlanet, PlanetToExplorer};
use common_game::utils::ID;
use crossbeam_channel::{Receiver, Sender};

#[derive(Clone, Copy)]
pub enum ExplorerKind {
    Miner,
    Ai,
    Smart,
}

pub const EXPLORER_ORDER: [ExplorerKind; 3] =
    [ExplorerKind::Miner, ExplorerKind::Ai, ExplorerKind::Smart];

impl ExplorerKind {
    pub fn name(self) -> &'static str {
        match self {
            ExplorerKind::Miner => "Miner",
            ExplorerKind::Ai => "AI",
            ExplorerKind::Smart => "Smart",
        }
    }
}

pub fn make_explorer(
    kind: ExplorerKind,
    id: ID,
    current_planet: ID,
    rx_orch: Receiver<OrchestratorToExplorer>,
    tx_orch: Sender<ExplorerToOrchestrator<BagContent>>,
    tx_planet: Sender<ExplorerToPlanet>,
    rx_planet: Receiver<PlanetToExplorer>,
) -> Box<dyn Explorer + Send> {
    match kind {
        ExplorerKind::Miner => {
            Box::new(MockExplorer::new(id, current_planet, rx_orch, tx_orch, tx_planet, rx_planet))
        }
        ExplorerKind::Ai => {
            Box::new(AiExplorer::new(id, current_planet, rx_orch, tx_orch, tx_planet, rx_planet))
        }
        ExplorerKind::Smart => {
            Box::new(SmartExplorer::new(id, current_planet, rx_orch, tx_orch, tx_planet, rx_planet))
        }
    }
}
