//! Events the orchestrator emits for the front-end to render.

use common_game::utils::ID;

#[derive(Debug, Clone, Copy)]
pub enum GuiEvent {
    SunrayReceived { planet: ID },
    AsteroidDeflected { planet: ID },
    PlanetDestroyed { planet: ID },
    ExplorerMoved { explorer: ID, to: ID },
}

#[derive(Default)]
pub struct EventBuffer {
    events: Vec<GuiEvent>,
}

impl EventBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, event: GuiEvent) {
        self.events.push(event);
    }

    /// Removes and returns all buffered events.
    pub fn drain(&mut self) -> Vec<GuiEvent> {
        std::mem::take(&mut self.events)
    }
}
