use common_game::protocols::messages::{
    ExplorerToPlanet, OrchestratorToPlanet, PlanetToExplorer, PlanetToOrchestrator,
};
use crossbeam_channel::{Receiver, Sender, unbounded};

type PlanetOrchHalfChannels = (Receiver<OrchestratorToPlanet>, Sender<PlanetToOrchestrator>);
type PlanetExplHalfChannels = (Receiver<ExplorerToPlanet>, Sender<PlanetToExplorer>);

fn get_test_channels() -> (PlanetOrchHalfChannels, PlanetExplHalfChannels) {
    let (_, rx_orch_in) = unbounded::<OrchestratorToPlanet>();
    let (tx_orch_out, _) = unbounded::<PlanetToOrchestrator>();
    let (_, rx_expl_in) = unbounded::<ExplorerToPlanet>();
    let (tx_expl_out, _) = unbounded::<PlanetToExplorer>();

    ((rx_orch_in, tx_orch_out), (rx_expl_in, tx_expl_out))
}

fn main() {
    let (orch_ch, expl_ch) = get_test_channels();
    let _planet = astro_parrot::create_planet(orch_ch.0, orch_ch.1, expl_ch.0, 1);
    println!("Planet created successfully");
}
