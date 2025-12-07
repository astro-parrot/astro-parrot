use common_game::components::planet::PlanetAI;
use common_game::components::planet::PlanetState;
use common_game::components::resource::BasicResource;
use common_game::components::resource::BasicResourceType;
use common_game::components::resource::ComplexResourceRequest;
use common_game::components::resource::{Combinator, Generator};
use common_game::components::rocket::Rocket;
use common_game::protocols::messages::{
    ExplorerToPlanet, OrchestratorToPlanet, PlanetToExplorer, PlanetToOrchestrator,
};

pub struct AstroParrot {
    pub active: bool,
}

impl PlanetAI for AstroParrot {
    fn handle_orchestrator_msg(
        &mut self,
        state: &mut PlanetState,
        _generator: &Generator,
        _combinator: &Combinator,
        msg: OrchestratorToPlanet,
    ) -> Option<PlanetToOrchestrator> {
        match msg {
            OrchestratorToPlanet::Sunray(sunray) => {
                let _ = state.charge_cell(sunray);

                // Always try to build a rocket when receiving sunray
                // to maximize planet survival chances
                if !state.has_rocket() {
                    match state.full_cell() {
                        Some((_, i)) => {
                            let _ = state.build_rocket(i);
                        }
                        None => {}
                    }
                }
                Some(PlanetToOrchestrator::SunrayAck {
                    planet_id: state.id(),
                })
            }
            OrchestratorToPlanet::KillPlanet => {
                self.stop(state);
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

    fn handle_asteroid(
        &mut self,
        state: &mut PlanetState,
        _generator: &Generator,
        _combinator: &Combinator,
    ) -> Option<Rocket> {
        match state.full_cell() {
            Some((_, i)) => {
                let _ = state.build_rocket(i);
            }
            None => {}
        }
        state.take_rocket()
    }

    fn handle_explorer_msg(
        &mut self,
        state: &mut PlanetState,
        generator: &Generator,
        combinator: &Combinator,
        msg: ExplorerToPlanet,
    ) -> Option<PlanetToExplorer> {
        match msg {
            ExplorerToPlanet::SupportedResourceRequest { explorer_id: _ } => {
                Some(PlanetToExplorer::SupportedResourceResponse {
                    resource_list: generator.all_available_recipes(),
                })
            }
            ExplorerToPlanet::SupportedCombinationRequest { explorer_id: _ } => {
                Some(PlanetToExplorer::SupportedCombinationResponse {
                    combination_list: combinator.all_available_recipes(),
                })
            }
            ExplorerToPlanet::GenerateResourceRequest {
                explorer_id: _,
                resource,
            } => {
                if let Some((cell, _)) = state.full_cell() {
                    match resource {
                        BasicResourceType::Carbon => {
                            if let Ok(carbon) = generator.make_carbon(cell) {
                                Some(PlanetToExplorer::GenerateResourceResponse {
                                    resource: Some(BasicResource::Carbon(carbon)),
                                })
                            } else {
                                Some(PlanetToExplorer::GenerateResourceResponse { resource: None })
                            }
                        }
                        BasicResourceType::Hydrogen => {
                            if let Ok(hydrogen) = generator.make_hydrogen(cell) {
                                Some(PlanetToExplorer::GenerateResourceResponse {
                                    resource: Some(BasicResource::Hydrogen(hydrogen)),
                                })
                            } else {
                                Some(PlanetToExplorer::GenerateResourceResponse { resource: None })
                            }
                        }
                        BasicResourceType::Oxygen => {
                            if let Ok(oxygen) = generator.make_oxygen(cell) {
                                Some(PlanetToExplorer::GenerateResourceResponse {
                                    resource: Some(BasicResource::Oxygen(oxygen)),
                                })
                            } else {
                                Some(PlanetToExplorer::GenerateResourceResponse { resource: None })
                            }
                        }
                        BasicResourceType::Silicon => {
                            if let Ok(silicon) = generator.make_silicon(cell) {
                                Some(PlanetToExplorer::GenerateResourceResponse {
                                    resource: Some(BasicResource::Silicon(silicon)),
                                })
                            } else {
                                Some(PlanetToExplorer::GenerateResourceResponse { resource: None })
                            }
                        }
                    }
                } else {
                    Some(PlanetToExplorer::GenerateResourceResponse { resource: None })
                }
            }
            ExplorerToPlanet::CombineResourceRequest {
                explorer_id: _,
                msg,
            } => {
                if let Some((cell, _)) = state.full_cell() {
                    match msg {
                        ComplexResourceRequest::Water(hydrogen, oxygen) => {
                            match combinator.make_water(hydrogen, oxygen, cell) {
                                Ok(water) => Some(PlanetToExplorer::CombineResourceResponse {
                                    complex_response: Ok(water.to_complex()),
                                }),
                                Err((e, h, o)) => Some(PlanetToExplorer::CombineResourceResponse {
                                    complex_response: Err((e, h.to_generic(), o.to_generic())),
                                }),
                            }
                        }
                        ComplexResourceRequest::AIPartner(robot, diamond) => {
                            match combinator.make_aipartner(robot, diamond, cell) {
                                Ok(ai_partner) => Some(PlanetToExplorer::CombineResourceResponse {
                                    complex_response: Ok(ai_partner.to_complex()),
                                }),
                                Err((e, r, d)) => Some(PlanetToExplorer::CombineResourceResponse {
                                    complex_response: Err((e, r.to_generic(), d.to_generic())),
                                }),
                            }
                        }
                        ComplexResourceRequest::Diamond(carbon1, carbon2) => {
                            match combinator.make_diamond(carbon1, carbon2, cell) {
                                Ok(diamond) => Some(PlanetToExplorer::CombineResourceResponse {
                                    complex_response: Ok(diamond.to_complex()),
                                }),
                                Err((e, c1, c2)) => {
                                    Some(PlanetToExplorer::CombineResourceResponse {
                                        complex_response: Err((
                                            e,
                                            c1.to_generic(),
                                            c2.to_generic(),
                                        )),
                                    })
                                }
                            }
                        }
                        ComplexResourceRequest::Dolphin(water, life) => {
                            match combinator.make_dolphin(water, life, cell) {
                                Ok(dolphin) => Some(PlanetToExplorer::CombineResourceResponse {
                                    complex_response: Ok(dolphin.to_complex()),
                                }),
                                Err((e, w, l)) => Some(PlanetToExplorer::CombineResourceResponse {
                                    complex_response: Err((e, w.to_generic(), l.to_generic())),
                                }),
                            }
                        }
                        ComplexResourceRequest::Life(water, carbon) => {
                            match combinator.make_life(water, carbon, cell) {
                                Ok(life) => Some(PlanetToExplorer::CombineResourceResponse {
                                    complex_response: Ok(life.to_complex()),
                                }),
                                Err((e, w, c)) => Some(PlanetToExplorer::CombineResourceResponse {
                                    complex_response: Err((e, w.to_generic(), c.to_generic())),
                                }),
                            }
                        }
                        ComplexResourceRequest::Robot(silicon, life) => {
                            match combinator.make_robot(silicon, life, cell) {
                                Ok(robot) => Some(PlanetToExplorer::CombineResourceResponse {
                                    complex_response: Ok(robot.to_complex()),
                                }),
                                Err((e, s, l)) => Some(PlanetToExplorer::CombineResourceResponse {
                                    complex_response: Err((e, s.to_generic(), l.to_generic())),
                                }),
                            }
                        }
                    }
                } else {
                    None
                }
            }
            ExplorerToPlanet::AvailableEnergyCellRequest { explorer_id: _ } => {
                let available_cells = state.cells_iter().filter(|c| c.is_charged()).count() as u32;
                Some(PlanetToExplorer::AvailableEnergyCellResponse { available_cells })
            }
        }
    }

    fn start(&mut self, _state: &PlanetState) {
        self.active = true;
    }

    fn stop(&mut self, _state: &PlanetState) {
        self.active = false;
    }
}
