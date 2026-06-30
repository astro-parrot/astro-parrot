//! Default autonomous [`Explorer`] implementation.
//!
//! The explorer is *reactive*: it blocks waiting for the orchestrator. A
//! [`OrchestratorToExplorer::BagContentRequest`] is its "turn" — during a turn it:
//!
//! 1. discovers what the current planet can do (cached until it travels);
//! 2. spends the planet's charged energy cells crafting toward the most valuable
//!    resource it can reach, walking the recipe tree one production step per cell;
//! 3. autonomously travels to a neighbouring planet (preferring unseen ones) when
//!    it cannot make progress here or it has stayed long enough to keep exploring;
//! 4. reports its bag.
//!
//! Every other orchestrator message is handled as a direct command, so the
//! explorer also works in the orchestrator's "manual" mode. The explorer never
//! panics: channel errors and timeouts degrade gracefully into "do nothing this
//! turn", which lets it survive unresponsive planets, relocation after a planet
//! is destroyed, and a shrinking galaxy.

use std::cmp::Reverse;
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
/// Force a travel after this many turns on the same planet, so the explorer roams.
const STAY_LIMIT: u32 = 3;
/// Upper bound on production steps in a single turn (one per charged cell, capped).
const MAX_OPS_PER_TURN: usize = 8;
/// How many copies of a "dead-end" basic resource to stockpile before stopping.
const MAX_BASIC_STOCK: usize = 3;

/// A single production action the explorer can take in one step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// Ask the planet to generate a basic resource.
    Generate(BasicResourceType),
    /// Ask the planet to combine resources from the bag into a complex one.
    Combine(ComplexResourceType),
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

    /// Travel bookkeeping.
    visited: HashSet<ID>,
    turns_here: u32,
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
            turns_here: 0,
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

    /// The explorer's turn: discover, produce, maybe travel, then report.
    fn take_turn(&mut self) {
        self.turns_here += 1;
        self.ensure_caps();

        let mut produced = false;
        for _ in 0..MAX_OPS_PER_TURN {
            match self.plan() {
                Some(Step::Generate(b)) if self.generate(b) => produced = true,
                Some(Step::Combine(c)) if self.combine(c) => produced = true,
                // A planned step that failed means the planet ran out of energy
                // (or there is nothing left to do). Stop producing this turn.
                _ => break,
            }
        }

        if !produced || self.turns_here >= STAY_LIMIT {
            self.travel();
        }
        self.report_bag();
    }

    /// Picks the single next production step, walking the recipe tree toward the
    /// most valuable complex resource the current planet can help build. Falls
    /// back to stockpiling a generatable basic (useful on a later planet).
    fn plan(&self) -> Option<Step> {
        let mut goals: Vec<ComplexResourceType> = self.combos.iter().copied().collect();
        goals.sort_by_key(|c| Reverse(value(*c)));
        for goal in goals {
            if let Some(step) = self.step_toward(goal) {
                return Some(step);
            }
        }
        // Nothing combinable here: stockpile a basic, bounded, to carry onward.
        self.gens
            .iter()
            .copied()
            .find(|&b| self.count(ResourceType::Basic(b)) < MAX_BASIC_STOCK)
            .map(Step::Generate)
    }

    /// The first action that makes progress toward crafting `goal` on this planet,
    /// or `None` if this planet cannot help. Recurses into sub-recipes.
    fn step_toward(&self, goal: ComplexResourceType) -> Option<Step> {
        if !self.combos.contains(&goal) {
            return None;
        }
        if self.can_combine_now(goal) {
            return Some(Step::Combine(goal));
        }
        for ingredient in self.missing(goal) {
            match ingredient {
                ResourceType::Basic(b) if self.gens.contains(&b) => return Some(Step::Generate(b)),
                ResourceType::Complex(c) => {
                    if let Some(step) = self.step_toward(c) {
                        return Some(step);
                    }
                }
                ResourceType::Basic(_) => {} // cannot generate it here; try next goal
            }
        }
        None
    }

    /// True if the bag already holds the ingredients to combine `c` right now.
    fn can_combine_now(&self, c: ComplexResourceType) -> bool {
        let [a, b] = recipe(c);
        if a == b {
            self.count(a) >= 2
        } else {
            self.count(a) >= 1 && self.count(b) >= 1
        }
    }

    /// The ingredients of `c` that are still missing from the bag.
    fn missing(&self, c: ComplexResourceType) -> Vec<ResourceType> {
        let [a, b] = recipe(c);
        if a == b {
            if self.count(a) >= 2 { Vec::new() } else { vec![a] }
        } else {
            [a, b].into_iter().filter(|&r| self.count(r) == 0).collect()
        }
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
    /// ingredients are not all present.
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
        self.turns_here = 0;
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

/// How desirable a complex resource is. Deeper recipes rank higher, so the
/// explorer aims for the richest product its planet can help build.
fn value(c: ComplexResourceType) -> u8 {
    use ComplexResourceType::*;
    match c {
        AIPartner => 6,
        Dolphin => 5,
        Robot => 4,
        Life => 3,
        Water => 2,
        Diamond => 1,
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for the explorer's decision logic.
    //!
    //! These exercise the planner in isolation, with no threads or planet. Real
    //! resource objects can only be minted by a planet, so the bag is empty here;
    //! that is enough to verify *which* production step the explorer chooses given
    //! a planet's capabilities. Autonomous crafting against a live planet is
    //! covered by the integration tests.

    use super::*;
    use crossbeam_channel::unbounded;

    use BasicResourceType::{Carbon, Hydrogen, Oxygen, Silicon};
    use ComplexResourceType::{AIPartner, Diamond, Life, Water};

    /// Builds an explorer with known capabilities and an empty bag.
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
    fn recipes_match_common_game_rules() {
        assert_eq!(recipe(Diamond), [ResourceType::Basic(Carbon), ResourceType::Basic(Carbon)]);
        assert_eq!(recipe(Water), [ResourceType::Basic(Hydrogen), ResourceType::Basic(Oxygen)]);
        assert_eq!(
            recipe(AIPartner),
            [ResourceType::Complex(ComplexResourceType::Robot), ResourceType::Complex(Diamond)]
        );
    }

    #[test]
    fn value_orders_deeper_recipes_higher() {
        assert!(value(AIPartner) > value(Water));
        assert!(value(Water) > value(Diamond));
    }

    #[test]
    fn generates_the_missing_basic_for_a_diamond() {
        let e = explorer_with(&[Carbon], &[Diamond]);
        assert_eq!(e.plan(), Some(Step::Generate(Carbon)));
    }

    #[test]
    fn descends_the_recipe_tree_toward_the_deepest_ingredient() {
        // Life = Water + Carbon; Water = Hydrogen + Oxygen. With an empty bag the
        // first concrete step is generating one of Water's gases.
        let e = explorer_with(&[Hydrogen, Oxygen, Carbon], &[Water, Life]);
        assert!(matches!(
            e.plan(),
            Some(Step::Generate(Hydrogen)) | Some(Step::Generate(Oxygen))
        ));
    }

    #[test]
    fn prefers_the_most_valuable_reachable_goal() {
        // Both Diamond and Water are reachable; Water outranks Diamond, so the
        // explorer works on Water (a gas), never starting from Carbon.
        let e = explorer_with(&[Carbon, Hydrogen, Oxygen], &[Diamond, Water]);
        assert!(matches!(
            e.plan(),
            Some(Step::Generate(Hydrogen)) | Some(Step::Generate(Oxygen))
        ));
    }

    #[test]
    fn still_progresses_when_top_goal_is_only_partly_reachable() {
        // AIPartner needs Robot (not combinable here) but its Diamond branch is,
        // so the planner keeps making progress by generating Carbon.
        let e = explorer_with(&[Carbon], &[Diamond, AIPartner]);
        assert_eq!(e.plan(), Some(Step::Generate(Carbon)));
    }

    #[test]
    fn stockpiles_a_basic_when_nothing_is_combinable_here() {
        let e = explorer_with(&[Silicon], &[]);
        assert_eq!(e.plan(), Some(Step::Generate(Silicon)));
    }

    #[test]
    fn does_nothing_useful_on_a_planet_it_cannot_use() {
        let e = explorer_with(&[], &[]);
        assert!(e.plan().is_none());
    }
}
