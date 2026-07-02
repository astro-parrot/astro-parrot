//! Orchestrator: galaxy topology, actor communication, planet/explorer
//! factories, and the core that drives them.

mod comm;
mod core;
mod events;
mod explorer_factory;
mod factory;
mod galaxy;

pub use core::{Command, Orchestrator};
pub use events::GuiEvent;
