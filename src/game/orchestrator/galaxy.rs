//! Galaxy topology: which planets exist and how they are connected.
//!
//! New planets are added fully connected to the existing ones, so an explorer
//! can travel from any planet to any other.

use std::collections::{HashMap, HashSet};

use common_game::utils::ID;

#[derive(Default)]
pub struct Galaxy {
    connections: HashMap<ID, HashSet<ID>>,
}

impl Galaxy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a planet connected to every existing planet.
    pub fn add_planet(&mut self, id: ID) {
        let existing: Vec<ID> = self.connections.keys().copied().collect();
        let mut neighbours = HashSet::new();
        for other in existing {
            neighbours.insert(other);
            if let Some(set) = self.connections.get_mut(&other) {
                set.insert(id);
            }
        }
        self.connections.insert(id, neighbours);
    }

    pub fn remove_planet(&mut self, id: ID) {
        self.connections.remove(&id);
        for neighbours in self.connections.values_mut() {
            neighbours.remove(&id);
        }
    }

    #[must_use]
    pub fn planets(&self) -> Vec<ID> {
        self.connections.keys().copied().collect()
    }

    #[must_use]
    pub fn are_connected(&self, a: ID, b: ID) -> bool {
        self.connections.get(&a).is_some_and(|n| n.contains(&b))
    }
}
