//! End-to-end test of the autonomous `AiExplorer`.
//!
//! A real AstroParrot planet (generates Carbon, combines Diamond) runs in one
//! thread, the explorer in another, and the test plays a minimal orchestrator:
//! it keeps the planet charged with sunrays and answers the explorer's
//! `NeighborsRequest`. With no command other than `StartExplorerAI`, the
//! explorer must explore, mine Carbon and craft a Diamond on its own.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use astro_parrot::{AiExplorer, BagContent, Explorer, create_planet};
use common_game::components::resource::{ComplexResourceType, ResourceType};
use common_game::components::sunray::Sunray;
use common_game::protocols::orchestrator_explorer::{ExplorerToOrchestrator, OrchestratorToExplorer};
use common_game::protocols::orchestrator_planet::{OrchestratorToPlanet, PlanetToOrchestrator};
use common_game::protocols::planet_explorer::{ExplorerToPlanet, PlanetToExplorer};
use crossbeam_channel::unbounded;

const STEP: Duration = Duration::from_millis(500);

#[test]
fn ai_explorer_explores_collects_and_crafts() {
    let planet_id = 1u32;
    let explorer_id = 2u32;

    // Planet channels.
    let (o2p_tx, o2p_rx) = unbounded::<OrchestratorToPlanet>();
    let (p2o_tx, p2o_rx) = unbounded::<PlanetToOrchestrator>();
    let (e2p_tx, e2p_rx) = unbounded::<ExplorerToPlanet>();

    // Explorer channels.
    let (o2e_tx, o2e_rx) = unbounded::<OrchestratorToExplorer>();
    let (e2o_tx, e2o_rx) = unbounded::<ExplorerToOrchestrator<BagContent>>();
    let (p2e_tx, p2e_rx) = unbounded::<PlanetToExplorer>();

    let mut planet = create_planet(o2p_rx, p2o_tx, e2p_rx, planet_id);
    let planet_handle = thread::spawn(move || {
        let _ = planet.run();
    });

    let mut explorer = AiExplorer::new(explorer_id, planet_id, o2e_rx, e2o_tx, e2p_tx, p2e_rx);
    let explorer_handle = thread::spawn(move || {
        let _ = explorer.run();
    });

    // Bring the planet online and register the explorer on it.
    o2p_tx.send(OrchestratorToPlanet::StartPlanetAI).unwrap();
    p2o_rx.recv_timeout(STEP).expect("planet did not start");
    o2p_tx
        .send(OrchestratorToPlanet::IncomingExplorerRequest { explorer_id, new_sender: p2e_tx })
        .unwrap();
    p2o_rx.recv_timeout(STEP).expect("planet did not accept explorer");

    // Minimal orchestrator: keep the planet charged, answer neighbour queries,
    // and track the latest reported bag.
    let stop = Arc::new(AtomicBool::new(false));
    let bag = Arc::new(Mutex::new(BagContent::default()));
    let driver = {
        let stop = Arc::clone(&stop);
        let bag = Arc::clone(&bag);
        let o2p_tx = o2p_tx.clone();
        let o2e_tx = o2e_tx.clone();
        thread::spawn(move || {
            let mut last_sun = Instant::now();
            let mut last_bag_req = Instant::now();
            while !stop.load(Ordering::Relaxed) {
                if last_sun.elapsed() >= Duration::from_millis(15) {
                    let _ = o2p_tx.send(OrchestratorToPlanet::Sunray(Sunray::default()));
                    let _ = p2o_rx.recv_timeout(Duration::from_millis(50)); // drain ack
                    last_sun = Instant::now();
                }
                if last_bag_req.elapsed() >= Duration::from_millis(30) {
                    let _ = o2e_tx.send(OrchestratorToExplorer::BagContentRequest);
                    last_bag_req = Instant::now();
                }
                match e2o_rx.recv_timeout(Duration::from_millis(5)) {
                    Ok(ExplorerToOrchestrator::NeighborsRequest { .. }) => {
                        // Single-planet galaxy: no neighbours to travel to.
                        let _ = o2e_tx.send(OrchestratorToExplorer::NeighborsResponse { neighbors: vec![] });
                    }
                    Ok(ExplorerToOrchestrator::BagContentResponse { bag_content, .. }) => {
                        *bag.lock().unwrap() = bag_content;
                    }
                    _ => {}
                }
            }
        })
    };

    // The only command the explorer needs.
    o2e_tx.send(OrchestratorToExplorer::StartExplorerAI).unwrap();

    // Wait until a Diamond shows up in the bag.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut crafted = false;
    while Instant::now() < deadline {
        let diamonds = bag
            .lock()
            .unwrap()
            .content
            .get(&ResourceType::Complex(ComplexResourceType::Diamond))
            .copied()
            .unwrap_or(0);
        if diamonds >= 1 {
            crafted = true;
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }

    // Tear everything down.
    stop.store(true, Ordering::Relaxed);
    let _ = driver.join();
    o2e_tx.send(OrchestratorToExplorer::KillExplorer).unwrap();
    o2p_tx.send(OrchestratorToPlanet::KillPlanet).unwrap();
    let _ = explorer_handle.join();
    let _ = planet_handle.join();

    assert!(crafted, "the AI explorer should have autonomously crafted a Diamond");
}
