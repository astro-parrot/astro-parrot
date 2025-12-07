use crate::helpers::{combine_complex, generate_basic};

use common_game::components::{
    planet::PlanetState,
    resource::{Combinator, Generator},
};
use common_game::protocols::messages::{ExplorerToPlanet, PlanetToExplorer};

pub fn handle(
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
