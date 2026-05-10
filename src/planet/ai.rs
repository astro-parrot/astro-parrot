use common_game::components::planet::{DummyPlanetState, PlanetAI, PlanetState};
use common_game::components::resource::{Combinator, Generator};
use common_game::components::rocket::Rocket;
use common_game::components::sunray::Sunray;
use common_game::protocols::planet_explorer::{ExplorerToPlanet, PlanetToExplorer};

use super::helpers::{combine_complex, generate_basic, try_build_rocket};

/// The `AstroParrot` Planet AI.
pub struct AstroParrotPlanetAI;

impl PlanetAI for AstroParrotPlanetAI {
    fn handle_sunray(
        &mut self,
        state: &mut PlanetState,
        _generator: &Generator,
        _combinator: &Combinator,
        sunray: Sunray,
    ) {
        let _ = state.charge_cell(sunray);

        if !state.has_rocket() {
            try_build_rocket(state);
        }
    }

    fn handle_asteroid(
        &mut self,
        state: &mut PlanetState,
        _generator: &Generator,
        _combinator: &Combinator,
    ) -> Option<Rocket> {
        try_build_rocket(state);
        state.take_rocket()
    }

    fn handle_internal_state_req(
        &mut self,
        state: &mut PlanetState,
        _generator: &Generator,
        _combinator: &Combinator,
    ) -> DummyPlanetState {
        state.to_dummy()
    }

    fn handle_explorer_msg(
        &mut self,
        state: &mut PlanetState,
        generator: &Generator,
        combinator: &Combinator,
        msg: ExplorerToPlanet,
    ) -> Option<PlanetToExplorer> {
        match msg {
            ExplorerToPlanet::SupportedResourceRequest { .. } => {
                Some(PlanetToExplorer::SupportedResourceResponse {
                    resource_list: generator.all_available_recipes(),
                })
            }

            ExplorerToPlanet::SupportedCombinationRequest { .. } => {
                Some(PlanetToExplorer::SupportedCombinationResponse {
                    combination_list: combinator.all_available_recipes(),
                })
            }

            ExplorerToPlanet::GenerateResourceRequest { resource, .. } => {
                if let Some((cell, _)) = state.full_cell() {
                    Some(PlanetToExplorer::GenerateResourceResponse {
                        resource: generate_basic(generator, cell, resource),
                    })
                } else {
                    Some(PlanetToExplorer::GenerateResourceResponse { resource: None })
                }
            }

            ExplorerToPlanet::CombineResourceRequest { msg, .. } => {
                if let Some((cell, _)) = state.full_cell() {
                    Some(PlanetToExplorer::CombineResourceResponse {
                        complex_response: combine_complex(combinator, cell, msg),
                    })
                } else {
                    None
                }
            }

            ExplorerToPlanet::AvailableEnergyCellRequest { .. } => {
                let available_cells = state.cells_iter().filter(|c| c.is_charged()).count() as u32;

                Some(PlanetToExplorer::AvailableEnergyCellResponse { available_cells })
            }
        }
    }
}
