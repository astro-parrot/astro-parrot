//! Builds planets from the various group implementations that share the
//! `common-game` interface. Each crate exposes its own creation function with a
//! slightly different signature/return type, which this factory normalises.

use std::time::Duration;

use common_game::components::planet::Planet;
use common_game::protocols::orchestrator_planet::{OrchestratorToPlanet, PlanetToOrchestrator};
use common_game::protocols::planet_explorer::ExplorerToPlanet;
use common_game::utils::ID;
use crossbeam_channel::{Receiver, Sender};

/// The planet implementations available in the galaxy.
#[derive(Clone, Copy)]
pub enum PlanetKind {
    AstroParrot,
    RustEze,
    OneMillionCrabs,
    Luna4,
    Trip,
    Skycartel,
    TheCompilerStrikesBack,
    ImmutableCosmicBorrow,
}

/// The fixed roster of planets that make up the galaxy (one AstroParrot planet
/// alongside seven planets from other groups).
pub const PLANET_ORDER: [PlanetKind; 8] = [
    PlanetKind::AstroParrot,
    PlanetKind::RustEze,
    PlanetKind::OneMillionCrabs,
    PlanetKind::Luna4,
    PlanetKind::Trip,
    PlanetKind::Skycartel,
    PlanetKind::TheCompilerStrikesBack,
    PlanetKind::ImmutableCosmicBorrow,
];

impl PlanetKind {
    /// A human-readable name for the planet's group.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            PlanetKind::AstroParrot => "AstroParrot",
            PlanetKind::RustEze => "Rust-eze",
            PlanetKind::OneMillionCrabs => "One Million Crabs",
            PlanetKind::Luna4 => "Luna4",
            PlanetKind::Trip => "TRIP",
            PlanetKind::Skycartel => "Skycartel",
            PlanetKind::TheCompilerStrikesBack => "The Compiler Strikes Back",
            PlanetKind::ImmutableCosmicBorrow => "Immutable Cosmic Borrow",
        }
    }
}

/// Creates a planet of the given kind, normalising the different group APIs to a
/// single `Result<Planet, String>`.
pub fn make_planet(
    kind: PlanetKind,
    id: ID,
    rx_orch: Receiver<OrchestratorToPlanet>,
    tx_orch: Sender<PlanetToOrchestrator>,
    rx_expl: Receiver<ExplorerToPlanet>,
) -> Result<Planet, String> {
    match kind {
        PlanetKind::AstroParrot => Ok(astro_parrot::create_planet(rx_orch, tx_orch, rx_expl, id)),
        PlanetKind::RustEze => Ok(rust_eze::create_planet(id, rx_orch, tx_orch, rx_expl)),
        PlanetKind::OneMillionCrabs => {
            one_million_crabs::planet::create_planet(rx_orch, tx_orch, rx_expl, id)
        }
        PlanetKind::Luna4 => luna4::create_planet(id, rx_orch, tx_orch, rx_expl),
        PlanetKind::Trip => trip::trip(id, rx_orch, tx_orch, rx_expl),
        PlanetKind::Skycartel => Ok(skycartel::create_planet(id, rx_orch, tx_orch, rx_expl)),
        PlanetKind::TheCompilerStrikesBack => {
            Ok(the_compiler_strikes_back::planet::create_planet(rx_orch, tx_orch, rx_expl, id))
        }
        PlanetKind::ImmutableCosmicBorrow => immutable_cosmic_borrow::create_planet(
            false,
            1.0,
            1.0,
            Duration::from_millis(100),
            Duration::from_secs(1),
            id,
            (rx_orch, tx_orch),
            rx_expl,
        ),
    }
}
