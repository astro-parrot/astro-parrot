mod explorer;
mod helpers;
mod orchestrator;

use common_game::components::{
    planet::{Planet, PlanetAI, PlanetState, PlanetType},
    resource::{BasicResourceType, Combinator, ComplexResourceType, Generator},
    rocket::Rocket,
};
use common_game::protocols::messages::{
    self, ExplorerToPlanet, OrchestratorToPlanet, PlanetToExplorer, PlanetToOrchestrator,
};
use crossbeam_channel::{Receiver, Sender};

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
        orchestrator::handle(state, generator, combinator, msg)
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

pub fn create_planet(
    rx_orchestrator: Receiver<messages::OrchestratorToPlanet>,
    tx_orchestrator: Sender<messages::PlanetToOrchestrator>,
    rx_explorer: Receiver<messages::ExplorerToPlanet>,
) -> Planet {
    use BasicResourceType::Carbon;
    use ComplexResourceType::{AIPartner, Diamond};

    let id = 1;
    let ai = AstroParrot { active: false };
    let gen_rules = vec![Carbon];
    let comb_rules = vec![AIPartner, Diamond];

    // Construct the planet and return it
    Planet::new(
        id,
        PlanetType::C,
        Box::new(ai),
        gen_rules,
        comb_rules,
        (rx_orchestrator, tx_orchestrator),
        rx_explorer,
    )
    .unwrap()
}
