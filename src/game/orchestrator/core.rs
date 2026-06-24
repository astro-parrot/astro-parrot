//! The orchestrator: spawns and drives planet and explorer threads.
//!
//! Planets are driven with the request/ack pattern. Explorers are *polled*: once
//! per cycle the orchestrator sends each explorer a `BagContentRequest`, which is
//! the explorer's turn. During that turn the explorer may autonomously ask to
//! move (`NeighborsRequest` / `TravelToPlanetRequest`), which the orchestrator
//! services before the explorer finally answers with its bag.

use std::collections::HashMap;
use std::thread::{self, JoinHandle};

use astro_parrot::{BagContent, Explorer, MockExplorer, create_planet};
use common_game::components::planet::DummyPlanetState;
use common_game::components::sunray::Sunray;
use common_game::components::asteroid::Asteroid;
use common_game::protocols::orchestrator_explorer::{
    ExplorerToOrchestrator, ExplorerToOrchestratorKind, OrchestratorToExplorer,
};
use common_game::protocols::orchestrator_planet::{OrchestratorToPlanet, PlanetToOrchestratorKind};
use common_game::protocols::planet_explorer::{ExplorerToPlanet, PlanetToExplorer};
use common_game::utils::ID;
use crossbeam_channel::{Sender, unbounded};

use super::comm::{ExplorerComm, PlanetComm};
use super::events::{EventBuffer, GuiEvent};
use super::galaxy::Galaxy;

/// Maximum number of planets allowed in the galaxy.
pub const MAX_PLANETS: usize = 8;

struct PlanetHandle {
    thread: JoinHandle<()>,
    tx_explorer: Sender<ExplorerToPlanet>,
}

struct ExplorerHandle {
    current_planet: ID,
    thread: JoinHandle<()>,
    tx_planet: Sender<PlanetToExplorer>,
}

/// A manual orchestrator action requested by the user, targeted at one planet.
pub enum Command {
    SendSunray { planet: ID },
    SendAsteroid { planet: ID },
}

pub struct Orchestrator {
    galaxy: Galaxy,
    planets: HashMap<ID, PlanetHandle>,
    explorers: HashMap<ID, ExplorerHandle>,
    planet_comm: PlanetComm,
    explorer_comm: ExplorerComm,
    planet_to_orch_tx: Sender<common_game::protocols::orchestrator_planet::PlanetToOrchestrator>,
    explorer_to_orch_tx: Sender<ExplorerToOrchestrator<BagContent>>,
    bags: HashMap<ID, BagContent>,
    events: EventBuffer,
    next_id: ID,
}

impl Orchestrator {
    #[must_use]
    pub fn new() -> Self {
        let (planet_to_orch_tx, planet_rx) = unbounded();
        let (explorer_to_orch_tx, explorer_rx) = unbounded();
        Self {
            galaxy: Galaxy::new(),
            planets: HashMap::new(),
            explorers: HashMap::new(),
            planet_comm: PlanetComm::new(planet_rx),
            explorer_comm: ExplorerComm::new(explorer_rx),
            planet_to_orch_tx,
            explorer_to_orch_tx,
            bags: HashMap::new(),
            events: EventBuffer::new(),
            next_id: 1,
        }
    }

    /// Spawns a new planet, starts its AI and wires it into the galaxy.
    pub fn add_planet(&mut self) -> Result<ID, String> {
        if self.planets.len() >= MAX_PLANETS {
            return Err("galaxy is full".to_string());
        }
        let id = self.next_id;
        self.next_id += 1;

        let (o2p_tx, o2p_rx) = unbounded::<OrchestratorToPlanet>();
        let (e2p_tx, e2p_rx) = unbounded::<ExplorerToPlanet>();
        let mut planet = create_planet(o2p_rx, self.planet_to_orch_tx.clone(), e2p_rx, id);
        let thread = thread::spawn(move || {
            let _ = planet.run();
        });

        self.planet_comm.register(id, o2p_tx);
        self.galaxy.add_planet(id);
        self.planets.insert(id, PlanetHandle { thread, tx_explorer: e2p_tx });

        self.planet_comm.req_ack(
            id,
            OrchestratorToPlanet::StartPlanetAI,
            PlanetToOrchestratorKind::StartPlanetAIResult,
        )?;
        Ok(id)
    }

    /// Spawns an explorer on `planet`, registers it there and starts its AI.
    pub fn add_explorer(&mut self, planet: ID) -> Result<ID, String> {
        if !self.planets.contains_key(&planet) {
            return Err(format!("planet {planet} does not exist"));
        }
        let id = self.next_id;
        self.next_id += 1;

        let (o2e_tx, o2e_rx) = unbounded::<OrchestratorToExplorer>();
        let (p2e_tx, p2e_rx) = unbounded::<PlanetToExplorer>();
        let first_planet_tx = self.planets[&planet].tx_explorer.clone();

        let mut explorer = MockExplorer::new(
            id,
            planet,
            o2e_rx,
            self.explorer_to_orch_tx.clone(),
            first_planet_tx,
            p2e_rx,
        );
        let thread = thread::spawn(move || {
            let _ = explorer.run();
        });

        self.explorer_comm.register(id, o2e_tx);
        self.explorers
            .insert(id, ExplorerHandle { current_planet: planet, thread, tx_planet: p2e_tx.clone() });

        self.notify_incoming(id, planet, p2e_tx)?;
        self.explorer_comm.req_ack(
            id,
            OrchestratorToExplorer::StartExplorerAI,
            ExplorerToOrchestratorKind::StartExplorerAIResult,
        )?;
        Ok(id)
    }

    /// Processes a manual action targeted at one planet.
    pub fn command(&mut self, command: Command) -> Result<(), String> {
        match command {
            Command::SendSunray { planet } => self.send_sunray(planet),
            Command::SendAsteroid { planet } => self.send_asteroid(planet).map(|_| ()),
        }
    }

    pub fn send_sunray(&mut self, planet: ID) -> Result<(), String> {
        self.planet_comm.req_ack(
            planet,
            OrchestratorToPlanet::Sunray(Sunray::default()),
            PlanetToOrchestratorKind::SunrayAck,
        )?;
        self.events.push(GuiEvent::SunrayReceived { planet });
        Ok(())
    }

    /// Sends an asteroid; returns `true` if the planet deflected it.
    pub fn send_asteroid(&mut self, planet: ID) -> Result<bool, String> {
        let rocket = self
            .planet_comm
            .req_ack(
                planet,
                OrchestratorToPlanet::Asteroid(Asteroid::default()),
                PlanetToOrchestratorKind::AsteroidAck,
            )?
            .into_asteroid_ack()
            .unwrap() // safe: kind checked above
            .1;

        if rocket.is_some() {
            self.events.push(GuiEvent::AsteroidDeflected { planet });
            Ok(true)
        } else {
            self.handle_planet_destroyed(planet)?;
            Ok(false)
        }
    }

    /// Gives every explorer a turn: it mines and may autonomously travel, then
    /// reports its bag.
    pub fn poll_explorers(&mut self) -> Result<(), String> {
        let ids: Vec<ID> = self.explorers.keys().copied().collect();
        for explorer in ids {
            self.explorer_comm
                .send_to(explorer, OrchestratorToExplorer::BagContentRequest)?;
            loop {
                match self.explorer_comm.recv_from(explorer)? {
                    ExplorerToOrchestrator::BagContentResponse { bag_content, .. } => {
                        self.bags.insert(explorer, bag_content);
                        break;
                    }
                    ExplorerToOrchestrator::NeighborsRequest { current_planet_id, .. } => {
                        let neighbors = self.galaxy.neighbours(current_planet_id);
                        self.explorer_comm.send_to(
                            explorer,
                            OrchestratorToExplorer::NeighborsResponse { neighbors },
                        )?;
                    }
                    ExplorerToOrchestrator::TravelToPlanetRequest {
                        current_planet_id,
                        dst_planet_id,
                        ..
                    } => {
                        self.handle_travel_request(explorer, current_planet_id, dst_planet_id)?;
                    }
                    other => {
                        return Err(format!("unexpected message from explorer {explorer}: {other:?}"));
                    }
                }
            }
        }
        Ok(())
    }

    fn handle_travel_request(&mut self, explorer: ID, current: ID, dst: ID) -> Result<(), String> {
        let sender = if self.galaxy.are_connected(current, dst) && self.planets.contains_key(&dst) {
            let p2e = self.explorers[&explorer].tx_planet.clone();
            self.notify_incoming(explorer, dst, p2e)?;
            self.notify_outgoing(explorer, current)?;
            Some(self.planets[&dst].tx_explorer.clone())
        } else {
            None
        };
        let moved = sender.is_some();

        self.explorer_comm.req_ack(
            explorer,
            OrchestratorToExplorer::MoveToPlanet { sender_to_new_planet: sender, planet_id: dst },
            ExplorerToOrchestratorKind::MovedToPlanetResult,
        )?;

        if moved {
            self.explorers.get_mut(&explorer).unwrap().current_planet = dst;
            self.events.push(GuiEvent::ExplorerMoved { explorer, to: dst });
        }
        Ok(())
    }

    // ----- queries ------------------------------------------------------

    #[must_use]
    pub fn alive_planets(&self) -> Vec<ID> {
        self.galaxy.planets()
    }

    #[must_use]
    pub fn neighbours(&self, planet: ID) -> Vec<ID> {
        self.galaxy.neighbours(planet)
    }

    #[must_use]
    pub fn explorers_on(&self, planet: ID) -> Vec<ID> {
        self.explorers
            .iter()
            .filter(|(_, h)| h.current_planet == planet)
            .map(|(&id, _)| id)
            .collect()
    }

    pub fn planet_state(&mut self, planet: ID) -> Option<DummyPlanetState> {
        if !self.planets.contains_key(&planet) {
            return None;
        }
        self.planet_comm
            .req_ack(
                planet,
                OrchestratorToPlanet::InternalStateRequest,
                PlanetToOrchestratorKind::InternalStateResponse,
            )
            .ok()
            .map(|res| res.into_internal_state_response().unwrap().1)
    }

    #[must_use]
    pub fn explorer_planet(&self, explorer: ID) -> Option<ID> {
        self.explorers.get(&explorer).map(|h| h.current_planet)
    }

    #[must_use]
    pub fn bag(&self, explorer: ID) -> Option<&BagContent> {
        self.bags.get(&explorer)
    }

    pub fn drain_events(&mut self) -> Vec<GuiEvent> {
        self.events.drain()
    }

    // ----- internals ----------------------------------------------------

    fn notify_incoming(
        &mut self,
        explorer: ID,
        planet: ID,
        new_sender: Sender<PlanetToExplorer>,
    ) -> Result<(), String> {
        let (_, accepted, res) = self
            .planet_comm
            .req_ack(
                planet,
                OrchestratorToPlanet::IncomingExplorerRequest { explorer_id: explorer, new_sender },
                PlanetToOrchestratorKind::IncomingExplorerResponse,
            )?
            .into_incoming_explorer_response()
            .unwrap(); // safe: kind checked above
        res.map_err(|e| format!("planet {planet} refused explorer {explorer}: {e}"))?;
        if accepted != explorer {
            return Err(format!("planet {planet} accepted the wrong explorer ({accepted})"));
        }
        Ok(())
    }

    fn notify_outgoing(&mut self, explorer: ID, planet: ID) -> Result<(), String> {
        self.planet_comm
            .req_ack(
                planet,
                OrchestratorToPlanet::OutgoingExplorerRequest { explorer_id: explorer },
                PlanetToOrchestratorKind::OutgoingExplorerResponse,
            )?
            .into_outgoing_explorer_response()
            .unwrap()
            .2
            .map_err(|e| format!("planet {planet} failed to release explorer {explorer}: {e}"))
    }

    fn handle_planet_destroyed(&mut self, planet: ID) -> Result<(), String> {
        self.galaxy.remove_planet(planet);
        self.kill_planet(planet)?;
        self.events.push(GuiEvent::PlanetDestroyed { planet });

        let stranded: Vec<ID> = self
            .explorers
            .iter()
            .filter(|(_, h)| h.current_planet == planet)
            .map(|(&id, _)| id)
            .collect();

        for explorer in stranded {
            if let Some(&dst) = self.galaxy.planets().first() {
                self.relocate_explorer(explorer, dst)?;
            } else {
                self.kill_explorer(explorer)?;
            }
        }
        Ok(())
    }

    fn relocate_explorer(&mut self, explorer: ID, dst: ID) -> Result<(), String> {
        let p2e = self.explorers[&explorer].tx_planet.clone();
        self.notify_incoming(explorer, dst, p2e)?;
        let new_planet_tx = self.planets[&dst].tx_explorer.clone();
        self.explorer_comm.req_ack(
            explorer,
            OrchestratorToExplorer::MoveToPlanet {
                sender_to_new_planet: Some(new_planet_tx),
                planet_id: dst,
            },
            ExplorerToOrchestratorKind::MovedToPlanetResult,
        )?;
        self.explorers.get_mut(&explorer).unwrap().current_planet = dst;
        self.events.push(GuiEvent::ExplorerMoved { explorer, to: dst });
        Ok(())
    }

    fn kill_planet(&mut self, planet: ID) -> Result<(), String> {
        if let Some(handle) = self.planets.remove(&planet) {
            self.planet_comm.req_ack(
                planet,
                OrchestratorToPlanet::KillPlanet,
                PlanetToOrchestratorKind::KillPlanetResult,
            )?;
            let _ = handle.thread.join();
            self.planet_comm.remove(planet);
        }
        Ok(())
    }

    fn kill_explorer(&mut self, explorer: ID) -> Result<(), String> {
        if let Some(handle) = self.explorers.remove(&explorer) {
            self.explorer_comm.req_ack(
                explorer,
                OrchestratorToExplorer::KillExplorer,
                ExplorerToOrchestratorKind::KillExplorerResult,
            )?;
            let _ = handle.thread.join();
            self.explorer_comm.remove(explorer);
        }
        Ok(())
    }
}

impl Default for Orchestrator {
    fn default() -> Self {
        Self::new()
    }
}
