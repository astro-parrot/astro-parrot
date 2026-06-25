//! End-to-end test of the autonomous `AiExplorer`.
//!
//! A real AstroParrot planet (generates Carbon, combines Diamond) runs in one
//! thread, the explorer in another, and the test plays a minimal **turn-based**
//! orchestrator that mirrors `Orchestrator::poll_explorers`: each turn it sends
//! the explorer a `BagContentRequest`, answers its `NeighborsRequest` /
//! `TravelToPlanetRequest`, and ends the turn when the explorer reports its bag.
//! With no command other than `StartExplorerAI`, the explorer must explore, mine
//! Carbon and craft a Diamond on its own.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use astro_parrot::{AiExplorer, BagContent, Explorer, create_planet};
use common_game::components::resource::{ComplexResourceType, ResourceType};
use common_game::components::sunray::Sunray;
use common_game::protocols::orchestrator_explorer::{ExplorerToOrchestrator, OrchestratorToExplorer};
use common_game::protocols::orchestrator_planet::{OrchestratorToPlanet, PlanetToOrchestrator};
use common_game::protocols::planet_explorer::{ExplorerToPlanet, PlanetToExplorer};
use crossbeam_channel::{Receiver, Sender, unbounded};

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

/// Charges a planet once and drains the ack.
fn pump_sunray(o2p: &Sender<OrchestratorToPlanet>, p2o: &Receiver<PlanetToOrchestrator>) {
    let _ = o2p.send(OrchestratorToPlanet::Sunray(Sunray::default()));
    let _ = p2o.recv_timeout(ACK);
}

/// Performs the planet/explorer handoff for a move, mirroring the real
/// `Orchestrator::handle_travel_request`.
#[allow(clippy::too_many_arguments)]
fn handoff(
    eid: u32,
    p2e_tx: &Sender<PlanetToExplorer>,
    dst_o2p: &Sender<OrchestratorToPlanet>,
    dst_p2o: &Receiver<PlanetToOrchestrator>,
    dst_e2p: &Sender<ExplorerToPlanet>,
    src_o2p: &Sender<OrchestratorToPlanet>,
    src_p2o: &Receiver<PlanetToOrchestrator>,
    o2e_tx: &Sender<OrchestratorToExplorer>,
    e2o_rx: &Receiver<ExplorerToOrchestrator<BagContent>>,
    dst_id: u32,
) {
    let _ = dst_o2p.send(OrchestratorToPlanet::IncomingExplorerRequest {
        explorer_id: eid,
        new_sender: p2e_tx.clone(),
    });
    let _ = dst_p2o.recv_timeout(ACK); // IncomingExplorerResponse
    let _ = src_o2p.send(OrchestratorToPlanet::OutgoingExplorerRequest { explorer_id: eid });
    let _ = src_p2o.recv_timeout(ACK); // OutgoingExplorerResponse
    let _ = o2e_tx.send(OrchestratorToExplorer::MoveToPlanet {
        sender_to_new_planet: Some(dst_e2p.clone()),
        planet_id: dst_id,
    });
    let _ = e2o_rx.recv_timeout(ACK); // MovedToPlanetResult
}

/// Two connected planets: the explorer must travel between them while exploring,
/// then still complete its task (craft a Diamond).
#[test]
fn ai_explorer_travels_between_planets() {
    let eid = 9u32;

    // Planet 1.
    let (o2p1_tx, o2p1_rx) = unbounded::<OrchestratorToPlanet>();
    let (p2o1_tx, p2o1_rx) = unbounded::<PlanetToOrchestrator>();
    let (e2p1_tx, e2p1_rx) = unbounded::<ExplorerToPlanet>();
    // Planet 2.
    let (o2p2_tx, o2p2_rx) = unbounded::<OrchestratorToPlanet>();
    let (p2o2_tx, p2o2_rx) = unbounded::<PlanetToOrchestrator>();
    let (e2p2_tx, e2p2_rx) = unbounded::<ExplorerToPlanet>();
    // Explorer.
    let (o2e_tx, o2e_rx) = unbounded::<OrchestratorToExplorer>();
    let (e2o_tx, e2o_rx) = unbounded::<ExplorerToOrchestrator<BagContent>>();
    let (p2e_tx, p2e_rx) = unbounded::<PlanetToExplorer>();

    let mut planet1 = create_planet(o2p1_rx, p2o1_tx, e2p1_rx, 1);
    let h1 = thread::spawn(move || {
        let _ = planet1.run();
    });
    let mut planet2 = create_planet(o2p2_rx, p2o2_tx, e2p2_rx, 2);
    let h2 = thread::spawn(move || {
        let _ = planet2.run();
    });

    // Explorer starts on planet 1.
    let mut explorer = AiExplorer::new(eid, 1, o2e_rx, e2o_tx, e2p1_tx.clone(), p2e_rx);
    let he = thread::spawn(move || {
        let _ = explorer.run();
    });

    // Start both planets and register the explorer on planet 1.
    o2p1_tx.send(OrchestratorToPlanet::StartPlanetAI).unwrap();
    p2o1_rx.recv_timeout(ACK).expect("planet 1 start");
    o2p2_tx.send(OrchestratorToPlanet::StartPlanetAI).unwrap();
    p2o2_rx.recv_timeout(ACK).expect("planet 2 start");
    o2p1_tx
        .send(OrchestratorToPlanet::IncomingExplorerRequest { explorer_id: eid, new_sender: p2e_tx.clone() })
        .unwrap();
    p2o1_rx.recv_timeout(ACK).expect("register on planet 1");

    o2e_tx.send(OrchestratorToExplorer::StartExplorerAI).unwrap();
    e2o_rx.recv_timeout(ACK).expect("explorer start");

    let stop = Arc::new(AtomicBool::new(false));
    let bag = Arc::new(Mutex::new(BagContent::default()));
    let travels = Arc::new(AtomicUsize::new(0));
    let mut current = 1u32;

    let driver = {
        let stop = Arc::clone(&stop);
        let bag = Arc::clone(&bag);
        let travels = Arc::clone(&travels);
        let o2e_tx = o2e_tx.clone();
        let o2p1_tx = o2p1_tx.clone();
        let o2p2_tx = o2p2_tx.clone();
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                pump_sunray(&o2p1_tx, &p2o1_rx);
                pump_sunray(&o2p2_tx, &p2o2_rx);
                if o2e_tx.send(OrchestratorToExplorer::BagContentRequest).is_err() {
                    break;
                }
                loop {
                    match e2o_rx.recv_timeout(ACK) {
                        Ok(ExplorerToOrchestrator::BagContentResponse { bag_content, .. }) => {
                            *bag.lock().unwrap() = bag_content;
                            break;
                        }
                        Ok(ExplorerToOrchestrator::NeighborsRequest { current_planet_id, .. }) => {
                            let other = if current_planet_id == 1 { 2 } else { 1 };
                            let _ = o2e_tx
                                .send(OrchestratorToExplorer::NeighborsResponse { neighbors: vec![other] });
                        }
                        Ok(ExplorerToOrchestrator::TravelToPlanetRequest { dst_planet_id, .. }) => {
                            if dst_planet_id == 2 && current == 1 {
                                handoff(eid, &p2e_tx, &o2p2_tx, &p2o2_rx, &e2p2_tx, &o2p1_tx, &p2o1_rx, &o2e_tx, &e2o_rx, 2);
                                current = 2;
                            } else if dst_planet_id == 1 && current == 2 {
                                handoff(eid, &p2e_tx, &o2p1_tx, &p2o1_rx, &e2p1_tx, &o2p2_tx, &p2o2_rx, &o2e_tx, &e2o_rx, 1);
                                current = 1;
                            } else {
                                let _ = o2e_tx.send(OrchestratorToExplorer::MoveToPlanet {
                                    sender_to_new_planet: None,
                                    planet_id: dst_planet_id,
                                });
                                let _ = e2o_rx.recv_timeout(ACK);
                                continue;
                            }
                            travels.fetch_add(1, Ordering::Relaxed);
                        }
                        _ => break,
                    }
                }
            }
        })
    };

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
        if diamonds >= 1 && travels.load(Ordering::Relaxed) >= 1 {
            crafted = true;
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }

    stop.store(true, Ordering::Relaxed);
    let _ = driver.join();
    o2e_tx.send(OrchestratorToExplorer::KillExplorer).unwrap();
    o2p1_tx.send(OrchestratorToPlanet::KillPlanet).unwrap();
    o2p2_tx.send(OrchestratorToPlanet::KillPlanet).unwrap();
    let _ = he.join();
    let _ = h1.join();
    let _ = h2.join();

    assert!(
        crafted,
        "explorer should have travelled between planets (travels={}) and crafted a Diamond",
        travels.load(Ordering::Relaxed)
    );
}
