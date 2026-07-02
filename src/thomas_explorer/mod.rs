//! Explorer abstraction.

mod smart_explorer;

use std::collections::HashMap;

use common_game::components::resource::ResourceType;
use common_game::protocols::orchestrator_explorer::{ExplorerToOrchestrator, OrchestratorToExplorer};
use common_game::protocols::planet_explorer::{ExplorerToPlanet, PlanetToExplorer};
use common_game::utils::ID;
use crossbeam_channel::{Receiver, Sender};

pub use smart_explorer::SmartExplorer;

/// The contents of an explorer's bag, reported back to the orchestrator/GUI.
#[derive(Debug, Clone, Default)]
pub struct BagContent {
    pub content: HashMap<ResourceType, usize>,
}

/// Behaviour of an explorer actor.
///
/// Implementors are constructed with the full set of channels they need and are
/// then driven by [`Explorer::run`], which blocks until the explorer is killed.
pub trait Explorer {
    /// Builds an explorer bound to its channels.
    fn new(
        id: ID,
        current_planet: ID,
        rx_orchestrator: Receiver<OrchestratorToExplorer>,
        tx_orchestrator: Sender<ExplorerToOrchestrator<BagContent>>,
        tx_current_planet: Sender<ExplorerToPlanet>,
        rx_planet: Receiver<PlanetToExplorer>,
    ) -> Self
    where
        Self: Sized;

    /// Runs the explorer's main loop until it is killed.
    ///
    /// # Errors
    /// Returns an error if a channel disconnects unexpectedly.
    fn run(&mut self) -> Result<(), String>;
}
