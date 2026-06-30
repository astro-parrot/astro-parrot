//! Autonomous [`Explorer`] implementation: an *obsessive Diamond collector*.
//!
//! This explorer cares about exactly one thing — Diamonds — and ignores every
//! other resource in the galaxy. On each turn (an
//! [`OrchestratorToExplorer::BagContentRequest`]) it:
//!
//! 1. discovers what the current planet can do (cached until it travels);
//! 2. makes a single step toward a Diamond: mine a Carbon, or fuse two Carbons
//!    into a Diamond (a Diamond's recipe is Carbon + Carbon);
//! 3. travels to a neighbouring planet when this one cannot help its obsession;
//! 4. once it owns [`TARGET_DIAMONDS`] Diamonds it stops collecting entirely and
//!    just roams the galaxy admiring its hoard ("museum mode").
//!
//! It also answers the orchestrator's manual commands, so it works in the
//! orchestrator's "manual" mode too. It never panics: channel errors and
//! timeouts degrade gracefully into "do nothing this turn", which lets it
//! survive unresponsive planets and a shrinking galaxy.

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

/// How long to wait for a planet to answer a request.
const PLANET_TIMEOUT: Duration = Duration::from_millis(200);
/// How long to wait for the orchestrator during the travel handshake.
const ORCH_TIMEOUT: Duration = Duration::from_millis(500);
/// The obsession: collect exactly this many Diamonds, then enter "museum mode".
const TARGET_DIAMONDS: usize = 5;

/// What the collector decides to do on a single turn. Kept separate from the
/// action itself so the decision logic stays pure and easy to unit-test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Move {
    /// Fuse two Carbons in the bag into a Diamond here.
    Forge,
    /// Mine one Carbon toward the next Diamond.
    Mine,
    /// Nothing to do here (planet useless, or collection complete): travel on.
    Wander,
}

pub struct SmartExplorer {
    id: ID,
    current_planet: ID,
    rx_orchestrator: Receiver<OrchestratorToExplorer>,
    tx_orchestrator: Sender<ExplorerToOrchestrator<BagContent>>,
    tx_planet: Sender<ExplorerToPlanet>,
    rx_planet: Receiver<PlanetToExplorer>,

    /// Real resource objects carried by the explorer (needed to craft).
    basics: Vec<BasicResource>,
    complexes: Vec<ComplexResource>,

    /// What the *current* planet can do, discovered on arrival.
    gens: HashSet<BasicResourceType>,
    combos: HashSet<ComplexResourceType>,
    caps_known: bool,

    /// Travel bookkeeping: which planets we've seen, and a cursor used to keep
    /// roaming when every neighbour has already been visited.
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
    // ----- the autonomous turn -----------------------------------------

    /// The collector's turn: discover, take one step toward a Diamond (or roam),
    /// then report the bag.
    fn take_turn(&mut self) {
        use BasicResourceType::Carbon;
        use ComplexResourceType::Diamond;

        self.ensure_caps();
        match self.decide() {
            // If the action fails (e.g. the planet ran out of energy) we just
            // move on; the bag is never left in a half-built state.
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

    /// Decides the single move for this turn, based only on what the bag holds
    /// and what the current planet can do. Pure (no I/O), so it is easy to test.
    fn decide(&self) -> Move {
        use BasicResourceType::Carbon;
        use ComplexResourceType::Diamond;

        // Obsession satisfied: the collection is complete. Stop producing and
        // just roam, admiring the hoard ("museum mode").
        if self.count(ResourceType::Complex(Diamond)) >= TARGET_DIAMONDS {
            return Move::Wander;
        }
        // Two Carbons in the bag and a planet that can fuse them: make a Diamond.
        if self.combos.contains(&Diamond) && self.count(ResourceType::Basic(Carbon)) >= 2 {
            return Move::Forge;
        }
        // Still short on Carbon and this planet can mine it: mine. Every other
        // resource the planet offers is deliberately ignored — only Diamonds.
        if self.gens.contains(&Carbon) && self.count(ResourceType::Basic(Carbon)) < 2 {
            return Move::Mine;
        }
        // This planet can't advance the collection: go find one that can.
        Move::Wander
    }

    // ----- planet interactions -----------------------------------------

    /// Discovers (once per planet) what the current planet can generate/combine.
    fn ensure_caps(&mut self) {
        if self.caps_known {
            return;
        }
        let gens = match self.planet_roundtrip(ExplorerToPlanet::SupportedResourceRequest {
            explorer_id: self.id,
        }) {
            Some(PlanetToExplorer::SupportedResourceResponse { resource_list }) => resource_list,
            _ => return, // leave unknown; retry next turn
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

    /// Number of charged energy cells the planet currently has.
    fn available_cells(&self) -> usize {
        match self.planet_roundtrip(ExplorerToPlanet::AvailableEnergyCellRequest { explorer_id: self.id }) {
            Some(PlanetToExplorer::AvailableEnergyCellResponse { available_cells }) => available_cells as usize,
            _ => 0,
        }
    }

    /// Asks the planet to generate a basic resource, storing it on success.
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

    /// Asks the planet to combine bag resources into `target`, storing the result.
    ///
    /// Checks for energy first: a planet with no charged cell silently drops the
    /// request, which would consume the ingredients without giving them back.
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

    /// Pulls the typed ingredients for `target` out of the bag and builds the
    /// combination request. Returns `None` (leaving the bag untouched) if the
    /// ingredients are not all present. Kept fully general so the orchestrator's
    /// manual "combine" command still works for any resource.
    fn build_request(&mut self, target: ComplexResourceType) -> Option<ComplexResourceRequest> {
        use BasicResourceType as B;
        use ComplexResourceType as C;

        // Guard on availability before removing anything, so a partial failure
        // never loses a resource.
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

    /// Asks the orchestrator for neighbours and moves to one, preferring planets
    /// not visited yet so the explorer keeps discovering the galaxy.
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

    /// Updates state after landing on a new planet.
    fn arrive(&mut self, planet: ID) {
        self.current_planet = planet;
        self.visited.insert(planet);
        self.caps_known = false;
        self.gens.clear();
        self.combos.clear();
    }

    // ----- direct (manual-mode) commands -------------------------------

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

    // ----- bag & inventory helpers -------------------------------------

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

    // ----- channel helpers ---------------------------------------------

    fn planet_roundtrip(&self, msg: ExplorerToPlanet) -> Option<PlanetToExplorer> {
        self.tx_planet.send(msg).ok()?;
        self.rx_planet.recv_timeout(PLANET_TIMEOUT).ok()
    }

    fn reply(&self, msg: ExplorerToOrchestrator<BagContent>) {
        let _ = self.tx_orchestrator.send(msg);
    }
}

/// The two ingredients of a complex resource, as in the common crate's rules.
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
    //! Unit tests for the collector's decision logic.
    //!
    //! These exercise [`SmartExplorer::decide`] in isolation, with no threads or
    //! planet. Real resource objects can only be minted by a planet, so the bag
    //! is empty here; that is enough to verify *which* move the collector picks
    //! given a planet's capabilities. The "Forge" and "museum mode" branches need
    //! a non-empty bag and are covered by the integration tests.

    use super::*;
    use crossbeam_channel::unbounded;

    use BasicResourceType::{Carbon, Hydrogen, Oxygen};
    use ComplexResourceType::{Diamond, Water};

    /// Builds a collector with known capabilities and an empty bag.
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
        // Empty bag, planet mines Carbon and forges Diamonds: first step is to mine.
        let e = explorer_with(&[Carbon], &[Diamond]);
        assert_eq!(e.decide(), Move::Mine);
    }

    #[test]
    fn mines_carbon_even_where_it_cannot_forge() {
        // No forge here, but Carbon is the only thing it wants — mine and carry on.
        let e = explorer_with(&[Carbon], &[]);
        assert_eq!(e.decide(), Move::Mine);
    }

    #[test]
    fn ignores_every_resource_that_is_not_carbon() {
        // The planet only offers Water and its gases — useless to a Diamond fiend.
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
