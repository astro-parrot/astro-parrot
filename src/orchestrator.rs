use crate::helpers::try_build_rocket;

use crate::AstroParrot;
use common_game::components::{
    planet::{PlanetAI, PlanetState},
    resource::{Combinator, Generator},
};
use common_game::protocols::messages::{OrchestratorToPlanet, PlanetToOrchestrator};

pub fn handle(
    ai: &mut AstroParrot,
    state: &mut PlanetState,
    _generator: &Generator,
    _combinator: &Combinator,
    msg: OrchestratorToPlanet,
) -> Option<PlanetToOrchestrator> {
    match msg {
        OrchestratorToPlanet::Sunray(sunray) => {
            let _ = state.charge_cell(sunray);

            if !state.has_rocket() {
                try_build_rocket(state);
            }

            Some(PlanetToOrchestrator::SunrayAck {
                planet_id: state.id(),
            })
        }

        OrchestratorToPlanet::KillPlanet => {
            ai.stop(state);
            Some(PlanetToOrchestrator::KillPlanetResult {
                planet_id: state.id(),
            })
        }

        OrchestratorToPlanet::InternalStateRequest => {
            Some(PlanetToOrchestrator::InternalStateResponse {
                planet_id: state.id(),
                planet_state: state.to_dummy(),
            })
        }

        _ => None,
    }
}
