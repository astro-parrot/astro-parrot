// What the explorer has learned about the galaxy, built up as it visits planets.

use std::collections::{HashMap, HashSet};

use common_game::components::resource::{BasicResourceType, ComplexResourceType};
use common_game::utils::ID;

#[derive(Clone)]
pub struct PlanetKnowledge {
    pub id: ID,
    pub neighbors: HashSet<ID>,
    pub basics: HashSet<BasicResourceType>,
    pub combinations: HashSet<ComplexResourceType>,
    pub charged_cells: ID,
    // Capabilities already queried.
    pub visited: bool,
    // Destroyed/stopped: excluded from travel and planning.
    pub dead: bool,
}

impl PlanetKnowledge {
    pub fn new(id: ID) -> Self {
        Self {
            id,
            neighbors: HashSet::new(),
            basics: HashSet::new(),
            combinations: HashSet::new(),
            charged_cells: 0,
            visited: false,
            dead: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrategyState {
    Exploring,
    Collecting,
    Crafting,
    Done,
}

#[derive(Default)]
pub struct ExplorerKnowledge {
    pub planets: HashMap<ID, PlanetKnowledge>,
}

impl ExplorerKnowledge {
    pub fn get(&self, id: ID) -> Option<&PlanetKnowledge> {
        self.planets.get(&id)
    }

    pub fn entry(&mut self, id: ID) -> &mut PlanetKnowledge {
        self.planets.entry(id).or_insert_with(|| PlanetKnowledge::new(id))
    }

    pub fn is_visited(&self, id: ID) -> bool {
        self.planets.get(&id).is_some_and(|p| p.visited)
    }

    pub fn is_dead(&self, id: ID) -> bool {
        self.planets.get(&id).is_some_and(|p| p.dead)
    }

    pub fn mark_dead(&mut self, id: ID) {
        let planet = self.entry(id);
        planet.dead = true;
        planet.visited = true;
    }
}
