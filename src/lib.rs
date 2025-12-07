mod explorer;
mod helpers;
mod orchestrator;

use common_game::components::{
    planet::{PlanetAI, PlanetState},
    resource::{Combinator, Generator},
    rocket::Rocket,
};
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
        generator: &Generator,
        combinator: &Combinator,
        msg: OrchestratorToPlanet,
    ) -> Option<PlanetToOrchestrator> {
        orchestrator::handle(self, state, generator, combinator, msg)
    }

    fn handle_asteroid(
        &mut self,
        state: &mut PlanetState,
        _generator: &Generator,
        _combinator: &Combinator,
    ) -> Option<Rocket> {
        helpers::try_build_rocket(state);
        state.take_rocket()
    }

    fn handle_explorer_msg(
        &mut self,
        state: &mut PlanetState,
        generator: &Generator,
        combinator: &Combinator,
        msg: ExplorerToPlanet,
    ) -> Option<PlanetToExplorer> {
        explorer::handle(state, generator, combinator, msg)
    }

    fn start(&mut self, _state: &PlanetState) {
        self.active = true;
    }

    fn stop(&mut self, _state: &PlanetState) {
        self.active = false;
    }
}
