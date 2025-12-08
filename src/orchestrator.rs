use crate::helpers::try_build_rocket;

use common_game::components::{
    planet::PlanetState,
    resource::{Combinator, Generator},
};
use common_game::protocols::messages::{OrchestratorToPlanet, PlanetToOrchestrator};

pub fn handle(
    state: &mut PlanetState,
    _generator: &Generator,
    _combinator: &Combinator,
    msg: OrchestratorToPlanet,
) -> Option<PlanetToOrchestrator> {
    match msg {
        OrchestratorToPlanet::Sunray(sunray) => {
            let _ = state.charge_cell(sunray);

            // Always try to build a rocket
            if !state.has_rocket() {
                try_build_rocket(state);
            }

            Some(PlanetToOrchestrator::SunrayAck {
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
