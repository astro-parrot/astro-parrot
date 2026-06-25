//! What the explorer has learned about the galaxy.
//!
//! The explorer builds this map incrementally as it visits planets and queries
//! their neighbours, the basic resources they can generate, the complex
//! resources they can combine, and how many charged energy cells they have.

use std::collections::{HashMap, HashSet};

use common_game::components::resource::{BasicResourceType, ComplexResourceType};
use common_game::utils::ID;

/// Everything the explorer knows about a single planet.
#[derive(Clone)]
pub struct PlanetKnowledge {
    pub id: ID,
    /// Planets reachable in one hop (reported by the orchestrator).
    pub neighbors: HashSet<ID>,
    /// Basic resources this planet can generate.
    pub basics: HashSet<BasicResourceType>,
    /// Complex resources this planet can combine.
    pub combinations: HashSet<ComplexResourceType>,
    /// Charged energy cells observed last time we asked.
    pub charged_cells: ID,
    /// Whether we have already queried this planet's capabilities.
    pub visited: bool,
    /// Whether the planet has been destroyed / stopped (excluded from planning).
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

/// The phase of the explorer's autonomous strategy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrategyState {
    /// Mapping the galaxy: visiting planets and recording their capabilities.
    Exploring,
    /// Gathering the basic resources required by the task.
    Collecting,
    /// Combining basics into the target complex resources.
    Crafting,
    /// Task finished (or no further progress possible).
    Done,
}

/// The explorer's whole view of the galaxy.
#[derive(Default)]
pub struct ExplorerKnowledge {
    pub planets: HashMap<ID, PlanetKnowledge>,
}

impl ExplorerKnowledge {
    /// Borrows the knowledge for a planet, if known.
    pub fn get(&self, id: ID) -> Option<&PlanetKnowledge> {
        self.planets.get(&id)
    }

    /// Borrows (creating a blank record if needed) the knowledge for a planet.
    pub fn entry(&mut self, id: ID) -> &mut PlanetKnowledge {
        self.planets.entry(id).or_insert_with(|| PlanetKnowledge::new(id))
    }

    /// Whether the planet's capabilities have already been queried.
    pub fn is_visited(&self, id: ID) -> bool {
        self.planets.get(&id).is_some_and(|p| p.visited)
    }

    /// Whether the planet is known to be destroyed / stopped.
    pub fn is_dead(&self, id: ID) -> bool {
        self.planets.get(&id).is_some_and(|p| p.dead)
    }

    /// Marks a planet as destroyed: excluded from travel and planning.
    pub fn mark_dead(&mut self, id: ID) {
        let planet = self.entry(id);
        planet.dead = true;
        planet.visited = true;
    }
}
