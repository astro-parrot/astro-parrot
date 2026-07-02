//! Autonomous explorer: an obsessive Diamond collector.
//!
//! Each turn it makes one move toward a Diamond — mine a Carbon, or fuse two
//! Carbons (Diamond = Carbon + Carbon). If the planet can't help, it travels on.
//! After `TARGET_DIAMONDS` Diamonds it stops crafting and just roams.
//!
//! It also answers the orchestrator's manual commands and never panics: channel
//! errors and timeouts just mean "do nothing this turn".

use std::collections::HashSet;
use std::time::Duration;

use common_game::components::resource::{
    BasicResource, BasicResourceType, ComplexResource, ComplexResourceRequest, ComplexResourceType,
    GenericResource, ResourceType,
};
use common_game::protocols::orchestrator_explorer::{ExplorerToOrchestrator, OrchestratorToExplorer};
use common_game::protocols::planet_explorer::{ExplorerToPlanet, PlanetToExplorer};
use common_game::utils::ID;
use crossbeam_channel::{Receiver, Sender};

use super::{BagContent, Explorer};

/// Max wait for a planet reply.
const PLANET_TIMEOUT: Duration = Duration::from_millis(200);
/// Max wait for the orchestrator during a travel handshake.
const ORCH_TIMEOUT: Duration = Duration::from_millis(500);
/// Diamonds to collect before going idle.
const TARGET_DIAMONDS: usize = 5;

/// This turn's move. Split from the action so `decide` stays pure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Move {
    Forge,  // fuse two Carbons into a Diamond
    Mine,   // mine one Carbon
    Wander, // planet useless or collection done: travel
}

pub struct SmartExplorer {
    id: ID,
    current_planet: ID,
    rx_orchestrator: Receiver<OrchestratorToExplorer>,
    tx_orchestrator: Sender<ExplorerToOrchestrator<BagContent>>,
    tx_planet: Sender<ExplorerToPlanet>,
    rx_planet: Receiver<PlanetToExplorer>,

    // Resources actually carried (needed to craft).
    basics: Vec<BasicResource>,
    complexes: Vec<ComplexResource>,

    // Current planet's capabilities, discovered on arrival.
    gens: HashSet<BasicResourceType>,
    combos: HashSet<ComplexResourceType>,
    caps_known: bool,

    // Travel bookkeeping.
    visited: HashSet<ID>,
    travel_seq: usize,
}

impl Explorer for SmartExplorer {
    fn new(
        id: ID,
        current_planet: ID,
        rx_orchestrator: Receiver<OrchestratorToExplorer>,
        tx_orchestrator: Sender<ExplorerToOrchestrator<BagContent>>,
        tx_current_planet: Sender<ExplorerToPlanet>,
        rx_planet: Receiver<PlanetToExplorer>,
    ) -> Self {
        let mut visited = HashSet::new();
        visited.insert(current_planet);
        Self {
            id,
            current_planet,
            rx_orchestrator,
            tx_orchestrator,
            tx_planet: tx_current_planet,
            rx_planet,
            basics: Vec::new(),
            complexes: Vec::new(),
            gens: HashSet::new(),
            combos: HashSet::new(),
            caps_known: false,
            visited,
            travel_seq: 0,
        }
    }

    fn run(&mut self) -> Result<(), String> {
        loop {
            match self.rx_orchestrator.recv() {
                Ok(OrchestratorToExplorer::KillExplorer) => {
                    self.reply(ExplorerToOrchestrator::KillExplorerResult { explorer_id: self.id });
                    return Ok(());
                }
                Ok(OrchestratorToExplorer::BagContentRequest) => self.take_turn(),
                Ok(msg) => self.handle(msg),
                Err(_) => return Ok(()), // orchestrator gone
            }
        }
    }
}

impl SmartExplorer {
    // ----- autonomous turn ---------------------------------------------

    /// One turn: discover, decide, act, report.
    fn take_turn(&mut self) {
        use BasicResourceType::Carbon;
        use ComplexResourceType::Diamond;

        self.ensure_caps();
        match self.decide() {
            // Action failed (e.g. no energy): move on instead of stalling.
            Move::Forge => {
                if !self.combine(Diamond) {
                    self.travel();
                }
            }
            Move::Mine => {
                if !self.generate(Carbon) {
                    self.travel();
                }
            }
            Move::Wander => self.travel(),
        }
        self.report_bag();
    }

    /// Picks this turn's move from the bag and the planet's capabilities. Pure.
    fn decide(&self) -> Move {
        use BasicResourceType::Carbon;
        use ComplexResourceType::Diamond;

        // Collection complete: stop and roam.
        if self.count(ResourceType::Complex(Diamond)) >= TARGET_DIAMONDS {
            return Move::Wander;
        }
        // Two Carbons in hand and a forge here.
        if self.combos.contains(&Diamond) && self.count(ResourceType::Basic(Carbon)) >= 2 {
            return Move::Forge;
        }
        // Need Carbon and can mine it here (everything else is ignored).
        if self.gens.contains(&Carbon) && self.count(ResourceType::Basic(Carbon)) < 2 {
            return Move::Mine;
        }
        // Planet can't help: travel.
        Move::Wander
    }

    // ----- planet interactions -----------------------------------------

    /// Discovers, once per planet, what it can generate/combine.
    fn ensure_caps(&mut self) {
        if self.caps_known {
            return;
        }
        let gens = match self.planet_roundtrip(ExplorerToPlanet::SupportedResourceRequest {
            explorer_id: self.id,
        }) {
            Some(PlanetToExplorer::SupportedResourceResponse { resource_list }) => resource_list,
            _ => return, // unknown: retry next turn
        };
        let combos = match self.planet_roundtrip(ExplorerToPlanet::SupportedCombinationRequest {
            explorer_id: self.id,
        }) {
            Some(PlanetToExplorer::SupportedCombinationResponse { combination_list }) => combination_list,
            _ => return,
        };
        self.gens = gens;
        self.combos = combos;
        self.caps_known = true;
    }

    /// Charged energy cells the planet has now.
    fn available_cells(&self) -> usize {
        match self.planet_roundtrip(ExplorerToPlanet::AvailableEnergyCellRequest { explorer_id: self.id }) {
            Some(PlanetToExplorer::AvailableEnergyCellResponse { available_cells }) => available_cells as usize,
            _ => 0,
        }
    }

    /// Generate a basic resource, keeping it on success.
    fn generate(&mut self, resource: BasicResourceType) -> bool {
        match self.planet_roundtrip(ExplorerToPlanet::GenerateResourceRequest {
            explorer_id: self.id,
            resource,
        }) {
            Some(PlanetToExplorer::GenerateResourceResponse { resource: Some(r) }) => {
                self.basics.push(r);
                true
            }
            _ => false,
        }
    }

    /// Combine bag resources into `target`, keeping the result. Bails if there's
    /// no charged cell — a dead planet drops the request and eats the ingredients.
    fn combine(&mut self, target: ComplexResourceType) -> bool {
        if self.available_cells() == 0 {
            return false;
        }
        let Some(request) = self.build_request(target) else {
            return false;
        };
        match self.planet_roundtrip(ExplorerToPlanet::CombineResourceRequest {
            explorer_id: self.id,
            msg: request,
        }) {
            Some(PlanetToExplorer::CombineResourceResponse { complex_response: Ok(c) }) => {
                self.complexes.push(c);
                true
            }
            Some(PlanetToExplorer::CombineResourceResponse { complex_response: Err((_, g1, g2)) }) => {
                self.restore(g1);
                self.restore(g2);
                false
            }
            _ => false,
        }
    }

    /// Takes the ingredients for `target` out of the bag and builds the request.
    /// `None` (bag untouched) if any is missing. Covers all recipes so the manual
    /// "combine" command still works.
    fn build_request(&mut self, target: ComplexResourceType) -> Option<ComplexResourceRequest> {
        use BasicResourceType as B;
        use ComplexResourceType as C;

        // Check availability before removing anything, so a failure loses nothing.
        let [a, b] = recipe(target);
        if a == b {
            if self.count(a) < 2 {
                return None;
            }
        } else if self.count(a) < 1 || self.count(b) < 1 {
            return None;
        }

        Some(match target {
            C::Diamond => {
                let c1 = self.take_basic(B::Carbon)?.to_carbon().ok()?;
                let c2 = self.take_basic(B::Carbon)?.to_carbon().ok()?;
                ComplexResourceRequest::Diamond(c1, c2)
            }
            C::Water => {
                let h = self.take_basic(B::Hydrogen)?.to_hydrogen().ok()?;
                let o = self.take_basic(B::Oxygen)?.to_oxygen().ok()?;
                ComplexResourceRequest::Water(h, o)
            }
            C::Life => {
                let w = self.take_complex(C::Water)?.to_water().ok()?;
                let c = self.take_basic(B::Carbon)?.to_carbon().ok()?;
                ComplexResourceRequest::Life(w, c)
            }
            C::Robot => {
                let s = self.take_basic(B::Silicon)?.to_silicon().ok()?;
                let l = self.take_complex(C::Life)?.to_life().ok()?;
                ComplexResourceRequest::Robot(s, l)
            }
            C::Dolphin => {
                let w = self.take_complex(C::Water)?.to_water().ok()?;
                let l = self.take_complex(C::Life)?.to_life().ok()?;
                ComplexResourceRequest::Dolphin(w, l)
            }
            C::AIPartner => {
                let r = self.take_complex(C::Robot)?.to_robot().ok()?;
                let d = self.take_complex(C::Diamond)?.to_diamond().ok()?;
                ComplexResourceRequest::AIPartner(r, d)
            }
        })
    }

    // ----- travel -------------------------------------------------------

    /// Asks for neighbours and moves, preferring unvisited planets.
    fn travel(&mut self) {
        if self
            .tx_orchestrator
            .send(ExplorerToOrchestrator::NeighborsRequest {
                explorer_id: self.id,
                current_planet_id: self.current_planet,
            })
            .is_err()
        {
            return;
        }

        let neighbours = match self.rx_orchestrator.recv_timeout(ORCH_TIMEOUT) {
            Ok(OrchestratorToExplorer::NeighborsResponse { neighbors }) => neighbors,
            _ => return,
        };
        if neighbours.is_empty() {
            return;
        }

        // Unvisited first; otherwise rotate through neighbours.
        let dst = neighbours
            .iter()
            .copied()
            .find(|n| !self.visited.contains(n))
            .unwrap_or_else(|| {
                let d = neighbours[self.travel_seq % neighbours.len()];
                self.travel_seq += 1;
                d
            });

        if self
            .tx_orchestrator
            .send(ExplorerToOrchestrator::TravelToPlanetRequest {
                explorer_id: self.id,
                current_planet_id: self.current_planet,
                dst_planet_id: dst,
            })
            .is_err()
        {
            return;
        }

        if let Ok(OrchestratorToExplorer::MoveToPlanet { sender_to_new_planet, planet_id }) =
            self.rx_orchestrator.recv_timeout(ORCH_TIMEOUT)
        {
            if let Some(tx) = sender_to_new_planet {
                self.tx_planet = tx;
                self.arrive(planet_id);
            }
            self.reply(ExplorerToOrchestrator::MovedToPlanetResult {
                explorer_id: self.id,
                planet_id: self.current_planet,
            });
        }
    }

    /// Reset state after landing on a new planet.
    fn arrive(&mut self, planet: ID) {
        self.current_planet = planet;
        self.visited.insert(planet);
        self.caps_known = false;
        self.gens.clear();
        self.combos.clear();
    }

    // ----- manual-mode commands ----------------------------------------

    fn handle(&mut self, msg: OrchestratorToExplorer) {
        match msg {
            OrchestratorToExplorer::StartExplorerAI => {
                self.reply(ExplorerToOrchestrator::StartExplorerAIResult { explorer_id: self.id });
            }
            OrchestratorToExplorer::StopExplorerAI => {
                self.reply(ExplorerToOrchestrator::StopExplorerAIResult { explorer_id: self.id });
            }
            OrchestratorToExplorer::ResetExplorerAI => {
                self.basics.clear();
                self.complexes.clear();
                self.caps_known = false;
                self.gens.clear();
                self.combos.clear();
                self.reply(ExplorerToOrchestrator::ResetExplorerAIResult { explorer_id: self.id });
            }
            OrchestratorToExplorer::CurrentPlanetRequest => {
                self.reply(ExplorerToOrchestrator::CurrentPlanetResult {
                    explorer_id: self.id,
                    planet_id: self.current_planet,
                });
            }
            OrchestratorToExplorer::MoveToPlanet { sender_to_new_planet, planet_id } => {
                if let Some(tx) = sender_to_new_planet {
                    self.tx_planet = tx;
                    self.arrive(planet_id);
                }
                self.reply(ExplorerToOrchestrator::MovedToPlanetResult {
                    explorer_id: self.id,
                    planet_id: self.current_planet,
                });
            }
            OrchestratorToExplorer::SupportedResourceRequest => {
                self.ensure_caps();
                self.reply(ExplorerToOrchestrator::SupportedResourceResult {
                    explorer_id: self.id,
                    supported_resources: self.gens.clone(),
                });
            }
            OrchestratorToExplorer::SupportedCombinationRequest => {
                self.ensure_caps();
                self.reply(ExplorerToOrchestrator::SupportedCombinationResult {
                    explorer_id: self.id,
                    combination_list: self.combos.clone(),
                });
            }
            OrchestratorToExplorer::GenerateResourceRequest { to_generate } => {
                let ok = self.generate(to_generate);
                self.reply(ExplorerToOrchestrator::GenerateResourceResponse {
                    explorer_id: self.id,
                    generated: ok.then_some(()).ok_or_else(|| "generation failed".to_string()),
                });
            }
            OrchestratorToExplorer::CombineResourceRequest { to_generate } => {
                let ok = self.combine(to_generate);
                self.reply(ExplorerToOrchestrator::CombineResourceResponse {
                    explorer_id: self.id,
                    generated: ok.then_some(()).ok_or_else(|| format!("cannot craft {to_generate:?}")),
                });
            }
            OrchestratorToExplorer::BagContentRequest => self.take_turn(),
            OrchestratorToExplorer::NeighborsResponse { .. } | OrchestratorToExplorer::KillExplorer => {}
        }
    }

    // ----- bag & inventory ---------------------------------------------

    fn report_bag(&self) {
        self.reply(ExplorerToOrchestrator::BagContentResponse {
            explorer_id: self.id,
            bag_content: self.bag(),
        });
    }

    fn bag(&self) -> BagContent {
        let mut bag = BagContent::default();
        for r in &self.basics {
            *bag.content.entry(ResourceType::Basic(r.get_type())).or_insert(0) += 1;
        }
        for r in &self.complexes {
            *bag.content.entry(ResourceType::Complex(r.get_type())).or_insert(0) += 1;
        }
        bag
    }

    fn count(&self, resource: ResourceType) -> usize {
        match resource {
            ResourceType::Basic(b) => self.basics.iter().filter(|r| r.get_type() == b).count(),
            ResourceType::Complex(c) => self.complexes.iter().filter(|r| r.get_type() == c).count(),
        }
    }

    fn take_basic(&mut self, basic: BasicResourceType) -> Option<BasicResource> {
        let i = self.basics.iter().position(|r| r.get_type() == basic)?;
        Some(self.basics.remove(i))
    }

    fn take_complex(&mut self, complex: ComplexResourceType) -> Option<ComplexResource> {
        let i = self.complexes.iter().position(|r| r.get_type() == complex)?;
        Some(self.complexes.remove(i))
    }

    fn restore(&mut self, resource: GenericResource) {
        match resource {
            GenericResource::BasicResources(b) => self.basics.push(b),
            GenericResource::ComplexResources(c) => self.complexes.push(c),
        }
    }

    // ----- channels ----------------------------------------------------

    fn planet_roundtrip(&self, msg: ExplorerToPlanet) -> Option<PlanetToExplorer> {
        self.tx_planet.send(msg).ok()?;
        self.rx_planet.recv_timeout(PLANET_TIMEOUT).ok()
    }

    fn reply(&self, msg: ExplorerToOrchestrator<BagContent>) {
        let _ = self.tx_orchestrator.send(msg);
    }
}

/// Complex-resource recipes, per the common crate.
fn recipe(c: ComplexResourceType) -> [ResourceType; 2] {
    use BasicResourceType as B;
    use ComplexResourceType as C;
    use ResourceType::{Basic, Complex};
    match c {
        C::Water => [Basic(B::Hydrogen), Basic(B::Oxygen)],
        C::Diamond => [Basic(B::Carbon), Basic(B::Carbon)],
        C::Life => [Complex(C::Water), Basic(B::Carbon)],
        C::Robot => [Basic(B::Silicon), Complex(C::Life)],
        C::Dolphin => [Complex(C::Water), Complex(C::Life)],
        C::AIPartner => [Complex(C::Robot), Complex(C::Diamond)],
    }
}

#[cfg(test)]
mod tests {
    // Test the decision logic in isolation. The bag can't be filled here (only a
    // planet mints resources), so these cover the mining/wander branches; the
    // forge and museum-mode branches are covered by the integration tests.

    use super::*;
    use crossbeam_channel::unbounded;

    use BasicResourceType::{Carbon, Hydrogen, Oxygen};
    use ComplexResourceType::{Diamond, Water};

    fn explorer_with(gens: &[BasicResourceType], combos: &[ComplexResourceType]) -> SmartExplorer {
        let (_o2e_tx, o2e_rx) = unbounded();
        let (e2o_tx, _e2o_rx) = unbounded();
        let (e2p_tx, _e2p_rx) = unbounded();
        let (_p2e_tx, p2e_rx) = unbounded();
        let mut e = SmartExplorer::new(1, 1, o2e_rx, e2o_tx, e2p_tx, p2e_rx);
        e.gens = gens.iter().copied().collect();
        e.combos = combos.iter().copied().collect();
        e.caps_known = true;
        e
    }

    #[test]
    fn mines_carbon_on_a_planet_that_can_forge_diamonds() {
        let e = explorer_with(&[Carbon], &[Diamond]);
        assert_eq!(e.decide(), Move::Mine);
    }

    #[test]
    fn mines_carbon_even_where_it_cannot_forge() {
        let e = explorer_with(&[Carbon], &[]);
        assert_eq!(e.decide(), Move::Mine);
    }

    #[test]
    fn ignores_every_resource_that_is_not_carbon() {
        let e = explorer_with(&[Hydrogen, Oxygen], &[Water]);
        assert_eq!(e.decide(), Move::Wander);
    }

    #[test]
    fn wanders_off_a_useless_planet() {
        let e = explorer_with(&[], &[]);
        assert_eq!(e.decide(), Move::Wander);
    }

    #[test]
    fn diamond_recipe_is_two_carbons() {
        assert_eq!(recipe(Diamond), [ResourceType::Basic(Carbon), ResourceType::Basic(Carbon)]);
    }
}
