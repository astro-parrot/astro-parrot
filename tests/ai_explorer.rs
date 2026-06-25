//! End-to-end test of the autonomous `AiExplorer`.
//!
//! A real AstroParrot planet (generates Carbon, combines Diamond) runs in one
//! thread, the explorer in another, and the test plays a minimal **turn-based**
//! orchestrator that mirrors `Orchestrator::poll_explorers`: each turn it sends
//! the explorer a `BagContentRequest`, answers its `NeighborsRequest` /
//! `TravelToPlanetRequest`, and ends the turn when the explorer reports its bag.
//! With no command other than `StartExplorerAI`, the explorer must explore, mine
//! Carbon and craft a Diamond on its own.

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

const ACK: Duration = Duration::from_millis(500);

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
    p2o_rx.recv_timeout(ACK).expect("planet did not start");
    o2p_tx
        .send(OrchestratorToPlanet::IncomingExplorerRequest { explorer_id, new_sender: p2e_tx })
        .unwrap();
    p2o_rx.recv_timeout(ACK).expect("planet did not accept explorer");

    // Start the explorer AI (the only command it needs), as the orchestrator
    // does on spawn.
    o2e_tx.send(OrchestratorToExplorer::StartExplorerAI).unwrap();
    e2o_rx.recv_timeout(ACK).expect("explorer did not start");

    // Minimal turn-based orchestrator running on its own thread.
    let stop = Arc::new(AtomicBool::new(false));
    let bag = Arc::new(Mutex::new(BagContent::default()));
    let driver = {
        let stop = Arc::clone(&stop);
        let bag = Arc::clone(&bag);
        let o2p_tx = o2p_tx.clone();
        let o2e_tx = o2e_tx.clone();
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                // Keep the planet's energy cells charged.
                for _ in 0..3 {
                    let _ = o2p_tx.send(OrchestratorToPlanet::Sunray(Sunray::default()));
                    let _ = p2o_rx.recv_timeout(ACK);
                }
                // One explorer turn.
                if o2e_tx.send(OrchestratorToExplorer::BagContentRequest).is_err() {
                    break;
                }
                loop {
                    match e2o_rx.recv_timeout(ACK) {
                        Ok(ExplorerToOrchestrator::BagContentResponse { bag_content, .. }) => {
                            *bag.lock().unwrap() = bag_content;
                            break;
                        }
                        Ok(ExplorerToOrchestrator::NeighborsRequest { .. }) => {
                            // Single-planet galaxy: no neighbours.
                            let _ = o2e_tx
                                .send(OrchestratorToExplorer::NeighborsResponse { neighbors: vec![] });
                        }
                        Ok(ExplorerToOrchestrator::TravelToPlanetRequest { dst_planet_id, .. }) => {
                            let _ = o2e_tx.send(OrchestratorToExplorer::MoveToPlanet {
                                sender_to_new_planet: None,
                                planet_id: dst_planet_id,
                            });
                            let _ = e2o_rx.recv_timeout(ACK); // consume MovedToPlanetResult
                        }
                        _ => break,
                    }
                }
            }
        })
    };

    // Wait until a Diamond shows up in the bag.
    let deadline = Instant::now() + Duration::from_secs(15);
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
