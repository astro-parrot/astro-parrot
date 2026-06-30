//! End-to-end test of the full actor pipeline:
//! orchestrator -> explorer -> planet -> explorer -> orchestrator, plus the
//! planet's asteroid defense. The test plays the role of the orchestrator with
//! raw channels, exactly as the real orchestrator wires things up.

use std::thread;
use std::time::Duration;

use astro_parrot::{SmartExplorer, BagContent, Explorer, create_planet};
use common_game::components::asteroid::Asteroid;
use common_game::components::resource::{BasicResourceType, ComplexResourceType, ResourceType};
use common_game::components::sunray::Sunray;
use common_game::protocols::orchestrator_explorer::{ExplorerToOrchestrator, OrchestratorToExplorer};
use common_game::protocols::orchestrator_planet::{OrchestratorToPlanet, PlanetToOrchestrator};
use common_game::protocols::planet_explorer::{ExplorerToPlanet, PlanetToExplorer};
use crossbeam_channel::{Receiver, unbounded};

const TIMEOUT: Duration = Duration::from_millis(500);

fn expect_explorer(rx: &Receiver<ExplorerToOrchestrator<BagContent>>) -> ExplorerToOrchestrator<BagContent> {
    rx.recv_timeout(TIMEOUT).expect("explorer did not answer")
}

#[test]
fn orchestrator_explorer_planet_pipeline() {
    let planet_id = 7u32;
    let explorer_id = 100u32;

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

    let mut explorer = SmartExplorer::new(
        explorer_id,
        planet_id,
        o2e_rx,
        e2o_tx,
        e2p_tx, // explorer talks to the planet here
        p2e_rx,
    );
    let explorer_handle = thread::spawn(move || {
        let _ = explorer.run();
    });

    // 1. Start the planet AI.
    o2p_tx.send(OrchestratorToPlanet::StartPlanetAI).unwrap();
    assert!(matches!(
        p2o_rx.recv_timeout(TIMEOUT).unwrap(),
        PlanetToOrchestrator::StartPlanetAIResult { .. }
    ));

    // 2. Register the explorer on the planet, then start the explorer AI.
    o2p_tx
        .send(OrchestratorToPlanet::IncomingExplorerRequest {
            explorer_id,
            new_sender: p2e_tx,
        })
        .unwrap();
    assert!(matches!(
        p2o_rx.recv_timeout(TIMEOUT).unwrap(),
        PlanetToOrchestrator::IncomingExplorerResponse { res: Ok(()), .. }
    ));

    o2e_tx.send(OrchestratorToExplorer::StartExplorerAI).unwrap();
    assert!(matches!(
        expect_explorer(&e2o_rx),
        ExplorerToOrchestrator::StartExplorerAIResult { .. }
    ));

    // Helper: send a sunray and wait for the ack.
    let sunray = || {
        o2p_tx.send(OrchestratorToPlanet::Sunray(Sunray::default())).unwrap();
        assert!(matches!(
            p2o_rx.recv_timeout(TIMEOUT).unwrap(),
            PlanetToOrchestrator::SunrayAck { .. }
        ));
    };

    // 3. First sunray builds a rocket; subsequent ones keep a cell charged.
    sunray();

    // Helper: drive a Carbon generation through the explorer.
    let gen_carbon = || {
        sunray();
        o2e_tx
            .send(OrchestratorToExplorer::GenerateResourceRequest {
                to_generate: BasicResourceType::Carbon,
            })
            .unwrap();
        match expect_explorer(&e2o_rx) {
            ExplorerToOrchestrator::GenerateResourceResponse { generated, .. } => {
                generated.expect("carbon generation failed");
            }
            other => panic!("unexpected explorer reply: {other:?}"),
        }
    };

    gen_carbon();
    gen_carbon();

    // 4. Craft a Diamond from the two Carbon resources.
    sunray();
    o2e_tx
        .send(OrchestratorToExplorer::CombineResourceRequest {
            to_generate: ComplexResourceType::Diamond,
        })
        .unwrap();
    match expect_explorer(&e2o_rx) {
        ExplorerToOrchestrator::CombineResourceResponse { generated, .. } => {
            generated.expect("diamond crafting failed");
        }
        other => panic!("unexpected explorer reply: {other:?}"),
    }

    // 5. The planet deflects an asteroid with the rocket built at step 3.
    o2p_tx.send(OrchestratorToPlanet::Asteroid(Asteroid::default())).unwrap();
    match p2o_rx.recv_timeout(TIMEOUT).unwrap() {
        PlanetToOrchestrator::AsteroidAck { rocket, .. } => {
            assert!(rocket.is_some(), "planet should have deflected the asteroid");
        }
        other => panic!("unexpected planet reply: {other:?}"),
    }

    // 6. Tear down both actors.
    o2e_tx.send(OrchestratorToExplorer::KillExplorer).unwrap();
    assert!(matches!(
        expect_explorer(&e2o_rx),
        ExplorerToOrchestrator::KillExplorerResult { .. }
    ));
    o2p_tx.send(OrchestratorToPlanet::KillPlanet).unwrap();
    assert!(matches!(
        p2o_rx.recv_timeout(TIMEOUT).unwrap(),
        PlanetToOrchestrator::KillPlanetResult { .. }
    ));

    assert!(explorer_handle.join().is_ok());
    assert!(planet_handle.join().is_ok());
}

/// Verifies an explorer autonomously initiates travel: when given turns
/// (BagContentRequest) it eventually asks for neighbours and requests a move,
/// completing the handshake. The test plays the orchestrator.
#[test]
fn explorer_autonomously_travels() {
    let planet_id = 1u32;
    let explorer_id = 50u32;

    // A real planet for the explorer to mine while taking its turns.
    let (o2p_tx, o2p_rx) = unbounded::<OrchestratorToPlanet>();
    let (p2o_tx, p2o_rx) = unbounded::<PlanetToOrchestrator>();
    let (e2p_tx, e2p_rx) = unbounded::<ExplorerToPlanet>();

    let mut planet = create_planet(o2p_rx, p2o_tx, e2p_rx, planet_id);
    let planet_handle = thread::spawn(move || {
        let _ = planet.run();
    });

    let (o2e_tx, o2e_rx) = unbounded::<OrchestratorToExplorer>();
    let (e2o_tx, e2o_rx) = unbounded::<ExplorerToOrchestrator<BagContent>>();
    let (p2e_tx, p2e_rx) = unbounded::<PlanetToExplorer>();

    let mut explorer = SmartExplorer::new(explorer_id, planet_id, o2e_rx, e2o_tx, e2p_tx, p2e_rx);
    let explorer_handle = thread::spawn(move || {
        let _ = explorer.run();
    });

    // Start planet, register and start the explorer.
    o2p_tx.send(OrchestratorToPlanet::StartPlanetAI).unwrap();
    p2o_rx.recv_timeout(TIMEOUT).unwrap();
    o2p_tx
        .send(OrchestratorToPlanet::IncomingExplorerRequest { explorer_id, new_sender: p2e_tx })
        .unwrap();
    p2o_rx.recv_timeout(TIMEOUT).unwrap();
    o2e_tx.send(OrchestratorToExplorer::StartExplorerAI).unwrap();
    expect_explorer(&e2o_rx);

    // A destination planet's request channel (kept alive so it stays connected).
    let (dst_tx, _dst_rx) = unbounded::<ExplorerToPlanet>();
    let dst_planet = 2u32;

    // Give the explorer turns until it autonomously travels.
    let mut moved = false;
    for _ in 0..8 {
        o2e_tx.send(OrchestratorToExplorer::BagContentRequest).unwrap();
        loop {
            match expect_explorer(&e2o_rx) {
                ExplorerToOrchestrator::BagContentResponse { .. } => break,
                ExplorerToOrchestrator::NeighborsRequest { current_planet_id, .. } => {
                    assert_eq!(current_planet_id, planet_id);
                    o2e_tx
                        .send(OrchestratorToExplorer::NeighborsResponse { neighbors: vec![dst_planet] })
                        .unwrap();
                }
                ExplorerToOrchestrator::TravelToPlanetRequest { dst_planet_id, .. } => {
                    assert_eq!(dst_planet_id, dst_planet);
                    o2e_tx
                        .send(OrchestratorToExplorer::MoveToPlanet {
                            sender_to_new_planet: Some(dst_tx.clone()),
                            planet_id: dst_planet,
                        })
                        .unwrap();
                }
                ExplorerToOrchestrator::MovedToPlanetResult { planet_id, .. } => {
                    assert_eq!(planet_id, dst_planet);
                    moved = true;
                }
                other => panic!("unexpected explorer message: {other:?}"),
            }
        }
        if moved {
            break;
        }
    }
    assert!(moved, "explorer never autonomously traveled");

    // Tear down.
    o2e_tx.send(OrchestratorToExplorer::KillExplorer).unwrap();
    assert!(explorer_handle.join().is_ok());
    o2p_tx.send(OrchestratorToPlanet::KillPlanet).unwrap();
    p2o_rx.recv_timeout(TIMEOUT).unwrap();
    assert!(planet_handle.join().is_ok());
}

/// Verifies the explorer's *autonomous brain*: given only its turns (and sunrays
/// to power the planet) it discovers the planet's recipes, mines Carbon and
/// crafts a Diamond on its own — no manual Generate/Combine commands. The test
/// plays the orchestrator and answers travel requests with no neighbours, so the
/// explorer stays put.
#[test]
fn explorer_autonomously_crafts_a_diamond() {
    let planet_id = 9u32;
    let explorer_id = 77u32;

    let (o2p_tx, o2p_rx) = unbounded::<OrchestratorToPlanet>();
    let (p2o_tx, p2o_rx) = unbounded::<PlanetToOrchestrator>();
    let (e2p_tx, e2p_rx) = unbounded::<ExplorerToPlanet>();

    let mut planet = create_planet(o2p_rx, p2o_tx, e2p_rx, planet_id);
    let planet_handle = thread::spawn(move || {
        let _ = planet.run();
    });

    let (o2e_tx, o2e_rx) = unbounded::<OrchestratorToExplorer>();
    let (e2o_tx, e2o_rx) = unbounded::<ExplorerToOrchestrator<BagContent>>();
    let (p2e_tx, p2e_rx) = unbounded::<PlanetToExplorer>();

    let mut explorer = SmartExplorer::new(explorer_id, planet_id, o2e_rx, e2o_tx, e2p_tx, p2e_rx);
    let explorer_handle = thread::spawn(move || {
        let _ = explorer.run();
    });

    // Start planet, register and start the explorer.
    o2p_tx.send(OrchestratorToPlanet::StartPlanetAI).unwrap();
    p2o_rx.recv_timeout(TIMEOUT).unwrap();
    o2p_tx
        .send(OrchestratorToPlanet::IncomingExplorerRequest { explorer_id, new_sender: p2e_tx })
        .unwrap();
    p2o_rx.recv_timeout(TIMEOUT).unwrap();
    o2e_tx.send(OrchestratorToExplorer::StartExplorerAI).unwrap();
    expect_explorer(&e2o_rx);

    let sunray = || {
        o2p_tx.send(OrchestratorToPlanet::Sunray(Sunray::default())).unwrap();
        assert!(matches!(
            p2o_rx.recv_timeout(TIMEOUT).unwrap(),
            PlanetToOrchestrator::SunrayAck { .. }
        ));
    };

    // First sunray is spent building the planet's defensive rocket; afterwards
    // each sunray leaves one charged cell for the explorer to use.
    sunray();

    let diamond = ResourceType::Complex(ComplexResourceType::Diamond);
    let mut crafted = false;
    for _ in 0..20 {
        sunray(); // one charged cell available for this turn
        o2e_tx.send(OrchestratorToExplorer::BagContentRequest).unwrap();
        loop {
            match expect_explorer(&e2o_rx) {
                ExplorerToOrchestrator::BagContentResponse { bag_content, .. } => {
                    if bag_content.content.get(&diamond).copied().unwrap_or(0) >= 1 {
                        crafted = true;
                    }
                    break;
                }
                ExplorerToOrchestrator::NeighborsRequest { .. } => {
                    o2e_tx
                        .send(OrchestratorToExplorer::NeighborsResponse { neighbors: vec![] })
                        .unwrap();
                }
                other => panic!("unexpected explorer message: {other:?}"),
            }
        }
        if crafted {
            break;
        }
    }
    assert!(crafted, "explorer never autonomously crafted a diamond");

    // Tear down.
    o2e_tx.send(OrchestratorToExplorer::KillExplorer).unwrap();
    assert!(explorer_handle.join().is_ok());
    o2p_tx.send(OrchestratorToPlanet::KillPlanet).unwrap();
    p2o_rx.recv_timeout(TIMEOUT).unwrap();
    assert!(planet_handle.join().is_ok());
}
