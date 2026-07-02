// The strategy: explore -> collect -> craft. advance() does one step per turn
// (at most one query/move/generate/combine) so the explorer stays responsive.

use std::collections::{HashMap, HashSet, VecDeque};

use common_game::components::resource::{
    BasicResourceType as B, ComplexResourceType, ResourceType,
};
use common_game::protocols::orchestrator_explorer::{
    ExplorerToOrchestrator, OrchestratorToExplorer, OrchestratorToExplorerKind,
};
use common_game::protocols::planet_explorer::{ExplorerToPlanet, PlanetToExplorer};
use common_game::utils::ID;

use super::AiExplorer;
use super::knowledge::StrategyState;
use super::recipes;

impl AiExplorer {
    pub(super) fn advance(&mut self) {
        self.drain_orchestrator();
        if !self.alive || !self.running {
            return;
        }
        // If we were moved somewhere new, learn it before using it.
        if self.state != StrategyState::Exploring && !self.knowledge.is_visited(self.current_planet) {
            self.query_planet(self.current_planet);
            return;
        }
        match self.state {
            StrategyState::Exploring => self.explore_step(),
            StrategyState::Collecting => self.collect_step(),
            StrategyState::Crafting => self.craft_step(),
            StrategyState::Done => self.maybe_rescan(),
        }
    }

    // Once done, keep an eye out for planets added after the first sweep; if one
    // shows up, go explore it (and retry resources that were impossible before).
    fn maybe_rescan(&mut self) {
        let current = self.current_planet;
        self.send_orchestrator(ExplorerToOrchestrator::NeighborsRequest {
            explorer_id: self.id,
            current_planet_id: current,
        });
        if let Some(OrchestratorToExplorer::NeighborsResponse { neighbors }) =
            self.await_orchestrator(OrchestratorToExplorerKind::NeighborsResponse, super::ORCH_TIMEOUT)
        {
            self.knowledge.entry(current).neighbors = neighbors.into_iter().collect();
        }
        if self.next_unvisited().is_some() {
            self.unobtainable.clear();
            self.stall = 0;
            self.state = StrategyState::Exploring;
        }
    }

    fn explore_step(&mut self) {
        let current = self.current_planet;
        if !self.knowledge.is_visited(current) {
            self.query_planet(current);
            return;
        }
        match self.next_unvisited() {
            Some(dst) => {
                if !self.travel_to(dst) {
                    self.unreachable.insert(dst);
                }
            }
            None => self.finish_exploration(),
        }
    }

    // Neighbours come from the orchestrator, capabilities and energy from the planet.
    fn query_planet(&mut self, id: ID) {
        self.send_orchestrator(ExplorerToOrchestrator::NeighborsRequest {
            explorer_id: self.id,
            current_planet_id: id,
        });
        if let Some(OrchestratorToExplorer::NeighborsResponse { neighbors }) =
            self.await_orchestrator(OrchestratorToExplorerKind::NeighborsResponse, super::ORCH_TIMEOUT)
        {
            self.knowledge.entry(id).neighbors = neighbors.into_iter().collect();
        }

        self.send_planet(ExplorerToPlanet::SupportedResourceRequest { explorer_id: self.id });
        if let Some(PlanetToExplorer::SupportedResourceResponse { resource_list }) =
            self.await_planet(super::PLANET_TIMEOUT)
        {
            self.knowledge.entry(id).basics = resource_list;
        }

        self.send_planet(ExplorerToPlanet::SupportedCombinationRequest { explorer_id: self.id });
        if let Some(PlanetToExplorer::SupportedCombinationResponse { combination_list }) =
            self.await_planet(super::PLANET_TIMEOUT)
        {
            self.knowledge.entry(id).combinations = combination_list;
        }

        self.send_planet(ExplorerToPlanet::AvailableEnergyCellRequest { explorer_id: self.id });
        if let Some(PlanetToExplorer::AvailableEnergyCellResponse { available_cells }) =
            self.await_planet(super::PLANET_TIMEOUT)
        {
            self.knowledge.entry(id).charged_cells = available_cells;
        }

        self.knowledge.entry(id).visited = true;
    }

    // Ask the orchestrator to move us; true once we've arrived.
    fn travel_to(&mut self, dst: ID) -> bool {
        if dst == self.current_planet {
            return true;
        }
        self.send_orchestrator(ExplorerToOrchestrator::TravelToPlanetRequest {
            explorer_id: self.id,
            current_planet_id: self.current_planet,
            dst_planet_id: dst,
        });
        match self.await_orchestrator(OrchestratorToExplorerKind::MoveToPlanet, super::ORCH_TIMEOUT) {
            Some(OrchestratorToExplorer::MoveToPlanet { sender_to_new_planet: Some(tx), planet_id }) => {
                self.tx_planet = tx;
                self.current_planet = planet_id;
                self.knowledge.entry(planet_id);
                self.send_orchestrator(ExplorerToOrchestrator::MovedToPlanetResult {
                    explorer_id: self.id,
                    planet_id,
                });
                true
            }
            Some(OrchestratorToExplorer::MoveToPlanet { sender_to_new_planet: None, .. }) => {
                // Refused; ack with our unchanged location.
                self.send_orchestrator(ExplorerToOrchestrator::MovedToPlanetResult {
                    explorer_id: self.id,
                    planet_id: self.current_planet,
                });
                false
            }
            _ => false,
        }
    }

    // BFS over known planets for the first hop toward the nearest unvisited one.
    fn next_unvisited(&self) -> Option<ID> {
        let mut queue = VecDeque::new();
        let mut seen = HashSet::new();
        let mut parent: HashMap<ID, ID> = HashMap::new();
        queue.push_back(self.current_planet);
        seen.insert(self.current_planet);

        while let Some(cur) = queue.pop_front() {
            let Some(pk) = self.knowledge.get(cur) else { continue };
            for &nb in &pk.neighbors {
                if self.knowledge.is_dead(nb) {
                    continue;
                }
                if !self.knowledge.is_visited(nb) && !self.unreachable.contains(&nb) {
                    return Some(self.first_hop(cur, nb, &parent));
                }
                if self.knowledge.is_visited(nb) && seen.insert(nb) {
                    parent.insert(nb, cur);
                    queue.push_back(nb);
                }
            }
        }
        None
    }

    fn first_hop(&self, cur: ID, nb: ID, parent: &HashMap<ID, ID>) -> ID {
        if cur == self.current_planet {
            return nb;
        }
        let mut node = cur;
        while let Some(&p) = parent.get(&node) {
            if p == self.current_planet {
                return node;
            }
            node = p;
        }
        nb
    }

    fn finish_exploration(&mut self) {
        if self.adaptive && self.task.is_empty() {
            let mut basics: HashSet<B> = HashSet::new();
            let mut combinations: HashSet<ComplexResourceType> = HashSet::new();
            for planet in self.knowledge.planets.values() {
                if planet.dead {
                    continue;
                }
                basics.extend(&planet.basics);
                combinations.extend(&planet.combinations);
            }
            // Aim only for what's actually craftable, so we don't gather basics
            // for an impossible goal.
            let feasible = recipes::feasible_complex(&basics, &combinations);
            if let Some((resource, count)) = recipes::pick_goal(&feasible) {
                self.task.insert(ResourceType::Complex(resource), count);
            }
        }
        self.state = StrategyState::Collecting;
        self.stall = 0;
    }

    fn collect_step(&mut self) {
        let needs = self.basic_needs();
        if needs.is_empty() {
            self.enter_crafting();
            return;
        }
        for &ty in needs.keys() {
            if self.unobtainable.contains(&ResourceType::Basic(ty)) {
                continue;
            }
            let planets = self.planets_generating(ty);
            if planets.is_empty() {
                self.unobtainable.insert(ResourceType::Basic(ty));
                continue;
            }
            let Some(dst) = self.choose_planet(&planets) else { continue };
            if self.current_planet != dst && !self.travel_to(dst) {
                self.unreachable.insert(dst);
                self.bump_stall_collect();
                return;
            }
            if self.generate_basic(ty) {
                self.depleted.clear();
                self.stall = 0;
            } else {
                self.depleted.insert(self.current_planet);
                self.bump_stall_collect();
            }
            return;
        }
        self.enter_crafting();
    }

    fn generate_basic(&mut self, ty: B) -> bool {
        self.send_planet(ExplorerToPlanet::GenerateResourceRequest { explorer_id: self.id, resource: ty });
        if let Some(PlanetToExplorer::GenerateResourceResponse { resource: Some(r) }) =
            self.await_planet(super::PLANET_TIMEOUT)
        {
            self.bag.add_basic(r);
            true
        } else {
            false
        }
    }

    // Basics still missing from the bag to satisfy the whole task.
    fn basic_needs(&self) -> HashMap<B, usize> {
        let mut need: HashMap<B, usize> = HashMap::new();
        for (rt, &count) in &self.task {
            match rt {
                ResourceType::Basic(b) => *need.entry(*b).or_default() += count,
                ResourceType::Complex(c) => recipes::basic_cost(*c, count, &mut need),
            }
        }
        let mut still = HashMap::new();
        for (b, n) in need {
            let have = self.bag.count_basic(b);
            if n > have {
                still.insert(b, n - have);
            }
        }
        still
    }

    fn craft_step(&mut self) {
        let targets = self.complex_targets();
        if targets.is_empty() || self.all_targets_met(&targets) {
            self.state = StrategyState::Done;
            return;
        }
        // Lowest tier first, so intermediates exist when a higher one needs them.
        let mut ordered: Vec<ComplexResourceType> = targets.keys().copied().collect();
        ordered.sort_by_key(|t| recipes::tier(*t));

        for ty in ordered {
            if self.produced.get(&ty).copied().unwrap_or(0) >= targets[&ty] {
                continue;
            }
            if self.unobtainable.contains(&ResourceType::Complex(ty)) {
                continue;
            }
            let planets = self.planets_combining(ty);
            if planets.is_empty() {
                self.unobtainable.insert(ResourceType::Complex(ty));
                continue;
            }
            if !self.has_ingredients(ty) {
                continue;
            }
            let Some(dst) = self.choose_planet(&planets) else { continue };
            if self.current_planet != dst && !self.travel_to(dst) {
                self.unreachable.insert(dst);
                self.bump_stall_craft();
                return;
            }
            if self.combine(ty) {
                self.depleted.clear();
                *self.produced.entry(ty).or_default() += 1;
                self.stall = 0;
            } else {
                self.depleted.insert(self.current_planet);
                self.bump_stall_craft();
            }
            return;
        }
        self.bump_stall_craft();
    }

    fn combine(&mut self, ty: ComplexResourceType) -> bool {
        // A planet with no charged cell silently drops the ingredients instead of
        // answering, so only hand them over once we know it has energy.
        if !self.planet_has_energy() {
            return false;
        }
        let Some(req) = recipes::build_request(&mut self.bag, ty) else {
            return false;
        };
        self.send_planet(ExplorerToPlanet::CombineResourceRequest { explorer_id: self.id, msg: req });
        match self.await_planet(super::PLANET_TIMEOUT) {
            Some(PlanetToExplorer::CombineResourceResponse { complex_response: Ok(c) }) => {
                self.bag.add_complex(c);
                true
            }
            Some(PlanetToExplorer::CombineResourceResponse { complex_response: Err((_, g1, g2)) }) => {
                self.recover(g1);
                self.recover(g2);
                false
            }
            _ => false,
        }
    }

    fn planet_has_energy(&mut self) -> bool {
        self.current_energy() >= 1
    }

    // Live energy of the current planet; also refreshes the stored estimate.
    fn current_energy(&mut self) -> ID {
        self.send_planet(ExplorerToPlanet::AvailableEnergyCellRequest { explorer_id: self.id });
        let cells = match self.await_planet(super::PLANET_TIMEOUT) {
            Some(PlanetToExplorer::AvailableEnergyCellResponse { available_cells }) => available_cells,
            _ => 0,
        };
        self.knowledge.entry(self.current_planet).charged_cells = cells;
        cells
    }

    // All complex resources to produce for the task, intermediates included.
    fn complex_targets(&self) -> HashMap<ComplexResourceType, usize> {
        let mut acc = HashMap::new();
        for (rt, &count) in &self.task {
            if let ResourceType::Complex(c) = rt {
                recipes::complex_production(*c, count, &mut acc);
            }
        }
        acc
    }

    fn all_targets_met(&self, targets: &HashMap<ComplexResourceType, usize>) -> bool {
        targets
            .iter()
            .all(|(ty, &n)| self.produced.get(ty).copied().unwrap_or(0) >= n)
    }

    fn has_ingredients(&self, ty: ComplexResourceType) -> bool {
        let (basics, complex) = recipes::ingredients(ty);
        let mut need_b: HashMap<B, usize> = HashMap::new();
        for b in basics {
            *need_b.entry(b).or_default() += 1;
        }
        let mut need_c: HashMap<ComplexResourceType, usize> = HashMap::new();
        for c in complex {
            *need_c.entry(c).or_default() += 1;
        }
        need_b.iter().all(|(b, &n)| self.bag.count_basic(*b) >= n)
            && need_c.iter().all(|(c, &n)| self.bag.count_complex(*c) >= n)
    }

    fn planets_generating(&self, ty: B) -> Vec<ID> {
        self.knowledge
            .planets
            .values()
            .filter(|p| !p.dead && p.basics.contains(&ty))
            .map(|p| p.id)
            .collect()
    }

    fn planets_combining(&self, ty: ComplexResourceType) -> Vec<ID> {
        self.knowledge
            .planets
            .values()
            .filter(|p| !p.dead && p.combinations.contains(&ty))
            .map(|p| p.id)
            .collect()
    }

    // Stay put if the current planet still has energy (cheap to check, no
    // travel); otherwise go to the candidate with the most known charged cells,
    // skipping dry ones. If all are dry, forget that and let them recharge.
    fn choose_planet(&mut self, candidates: &[ID]) -> Option<ID> {
        if candidates.is_empty() {
            return None;
        }
        let current = self.current_planet;
        if candidates.contains(&current)
            && !self.depleted.contains(&current)
            && self.current_energy() >= 1
        {
            return Some(current);
        }
        let mut pool: Vec<ID> = candidates
            .iter()
            .copied()
            .filter(|id| !self.depleted.contains(id))
            .collect();
        if pool.is_empty() {
            self.depleted.clear();
            pool = candidates.to_vec();
        }
        pool.into_iter().max_by_key(|&id| {
            let cells = self.knowledge.get(id).map_or(0, |p| p.charged_cells);
            (cells, ID::from(id == current))
        })
    }

    fn enter_crafting(&mut self) {
        self.state = StrategyState::Crafting;
        self.stall = 0;
    }

    fn bump_stall_collect(&mut self) {
        self.stall += 1;
        if self.stall >= super::MAX_STALL {
            self.enter_crafting();
        }
    }

    fn bump_stall_craft(&mut self) {
        self.stall += 1;
        if self.stall >= super::MAX_STALL {
            self.state = StrategyState::Done;
            self.stall = 0;
        }
    }
}
