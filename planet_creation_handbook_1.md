> [!note]
> This document comes directly from **WGL** repository.
> It is commited here only for pure convenience of **AstroParrot** group.
> Please refer to the original source for the most up-to-date version

# Planet creation handbook 1

This is an addendum of the official documentation aimed at aiding groups when implementing a planet.
This is written for common crate's version 1.0

If you are having issues with  migrating from a version prior to 1.0, please check  [Breaking changes in the last commit](https://github.com/lucaFiorini/AP_project_utils/blob/main/planet_creation_handbook_1.md#breaking-changes-in-the-last-commit).
# Planet module 
The `Planet` defined in the Common code provides the base for all implemented planets, it holds the internal state and handles the main execution loop of the planet.
What groups need to build is the `PlanetAI`, this is defined as a Trait for you to implement in a library crate.
A `dyn PlanetAI` object is used when constructing a `Planet`.
The polling loop for messages is implemented in the common code, groups will only need to handle certain messages as listed below in [An overlook of Planet Ai's methods](https://github.com/lucaFiorini/AP_project_utils/blob/main/planet_creation_handbook_1.md#an-overlook-of-planet-ais-methods)
# What your Crate should look like

Your crate should be very simple, it needs to:
- be a Library Crate
- have your Group's name
- hold a `pub struct <GroupName>` 
- an `impl` block for `<GroupName>` to implement `PlanetAI`
- a  public function with The following signature
```rust
//This function will be called by the Orchestrator
pub fn create_planet(
	rx_orchestrator: crossbeam::Receiver<messages::OrchestratorToPlanet>,
	tx_orchestrator: crossbeam::Sender<messages::PlanetToOrchestrator>,
	rx_explorer: crossbeam::Receiver<messages::ExplorerToPlanet>,
	planet_id: u32
) -> Planet {
	//Your code...
}
```

>[!note]
>In the example provided by the source file, `planet_id` is not included in create_planet.
>This is a mistake, as it's needed for the agreed-to id managment, where each group would manually assign an id to each planet.

>[!warning]
>The entirety of create_planet may be changed and added to a trait, as to provide a more standardised construction method.
>See [Issue 100](https://github.com/unitn-ap-2025/common/issues/100)

>[!note]
>`Planet::new()`, as of 1.0, returns a Result
> It will return `Err(_)` when the planet provided invalid data depending on the PlanetType
>As your planet's type and attributes should be static and unchanging, you should simply unwrap it.

>[!warning] 
>This may be fixed in a later vetsion
>See [Issue 98](https://github.com/unitn-ap-2025/common/issues/98)

---
## An overlook of Planet Ai's methods
These are the methods you are required to implement

### Message Handles 
`handle_orchestrator_msg` and `handle_explorer_msg` 

These two methods are the core of your AI.
They will provide the related message for you to handle, ideally, in a `match` statement.

When implementing these, you should resolve the request by sending expected responses to the relevant Parties using the relevant `crossbeam::Sender`s.

>[!note] 
>The following messages are handled in the common code, and you will never receive them through `handle_orchestrator_msg`
> - `StartPlanetAI`,
> - `StopPlanetAI`,
> - `KillPlanet`,
> - `Asteroid`,
> - `IncomingExplorerRequest`,
> - `OutgoingExplorerRequest`,
>
> To handle these messages, it is suggested to use a default case and ignore them.

>[!warning] 
>This architecture is not ideal, and may be changed in a later version.
>See [Issue 97](https://github.com/unitn-ap-2025/common/issues/97)

### Handling an Asteroid
> [!warning] 
> This is subject to change depending on the outcome of [Issue 97](https://github.com/unitn-ap-2025/common/issues/97)

Asteroids are not handled by [Message Handles](https://github.com/lucaFiorini/AP_project_utils/blob/main/planet_creation_handbook_1.md#message-handles), they have a special method,`handle_asteroid`

Within this function you are expected to extract a `Rocket` from the `PlanetState`.
and return it if you have it available, otherwise return `None`.
I.E. you should `take` the `Rocket` from `PlanetState`.
After you return, depending on the return value, the common crate's `Planet` will send relevant messages to the `Orchestrator`.
### Planet Start and Planet Stop
>[!warning] 
> This is subject to change depending on the outcome of [Issue 97](https://github.com/unitn-ap-2025/common/issues/97)

Planet Start and Planet Stop are not handled by [Message Handles](https://github.com/lucaFiorini/AP_project_utils/blob/main/planet_creation_handbook_1.md#message-handles),they have a special methods, `start` and `stop`

They allow you to change your AI's internal state when the planet starts or stops.

>[!caution]
>These methods **do not start or stop the planet themselves**, this behavior is defined in the common code.

>[!note]
>Due to the fact that the AI does not have to handle Start/Stop, you should not worry about getting any requests when stopped, these are redirected appropriately in the common code.

# Breaking changes in the last commit.
This section is for those who had implemented a working planet before the last commit which merged this [PR](https://github.com/unitn-ap-2025/common/pull/95).

It introduced a few breaking changes.
Specifically:
- library for Senders and Receivers has changed
	- We now use`crossbeam::Send` and `crossbeam::Recieve` instead of the implementations from `sync::mpsc`
- The following messages have been introduced to `OrchestratorToPlanet`
	- `KillPlanet`
- The following messages have been introduced to `PlanetToOrchestrator`
	- `KillPlanetResult`
- The following message's contents have changed 
	- `AsteroidAck` now contains an `Option<Rocket>` instead of `destroyed : bool` to signal successful asteroid deflection. The  `Orchestrator` is expected to kill a planet if it receives `None` rockets from `AsteroidAck`

