use common_game::components::planet::{Planet, PlanetType};
use common_game::components::resource::BasicResourceType::Carbon;
use common_game::components::resource::ComplexResourceType::{AIPartner, Diamond};
use common_game::protocols::{orchestrator_planet, planet_explorer};
use crossbeam_channel::{Receiver, Sender};

use super::ai::AstroParrotPlanetAI;

/// Creates and initializes the `AstroParrot` planet instance.
///
/// # Panics
/// Panics if the planet creation fails. This should not happen with the static
/// configuration used by AstroParrot.
#[must_use]
pub fn create_planet(
    rx_orchestrator: Receiver<orchestrator_planet::OrchestratorToPlanet>,
    tx_orchestrator: Sender<orchestrator_planet::PlanetToOrchestrator>,
    rx_explorer: Receiver<planet_explorer::ExplorerToPlanet>,
    planet_id: u32,
) -> Planet {
    let ai = AstroParrotPlanetAI;
    let gen_rules = vec![Carbon];
    let comb_rules = vec![AIPartner, Diamond];

    Planet::new(
        planet_id,
        PlanetType::C,
        Box::new(ai),
        gen_rules,
        comb_rules,
        (rx_orchestrator, tx_orchestrator),
        rx_explorer,
    )
    .expect("Failed to create the AstroParrot planet")
}
