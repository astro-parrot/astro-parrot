# AstroParrot

AstroParrot is our entry for the Advanced Programming 2025 project. It contains:

- a **planet** implementation (the library part), built on the `common-game`
  crate, and
- a **galaxy simulation** (the binary, behind the `game` feature) that runs our
  planet next to seven other groups' planets, together with three explorer
  implementations written by our members.

The library can be depended on on its own; the game is only pulled in when the
`game` feature is enabled (it is on by default).

## Layout

```
src/
  planet/            our planet AI and create_planet
  explorer/          the Explorer trait and three implementations
    mock_explorer.rs   MockExplorer  ("Miner")
    marco_explorer/    AiExplorer    ("AI")
    thomas_explorer/   SmartExplorer ("Smart")
  game/              macroquad front-end
    orchestrator/    spawns and drives the planet/explorer threads
  main.rs
  lib.rs
```

## The planet

`create_planet` builds a type **C** planet with one energy cell. It generates
`Carbon` and can combine `AIPartner` and `Diamond`.

The AI is deliberately defensive:

- **Sunray**: charge the cell, then build a rocket if we don't already have one.
  A rocket is kept ready at all times, so a single asteroid never catches the
  planet undefended.
- **Asteroid**: build a rocket if the cell allows it and hand it over. Returning
  a rocket is what lets the planet survive the hit.
- **Explorer requests**: report the generation and combination recipes, and
  serve `GenerateResourceRequest` / `CombineResourceRequest` whenever a cell is
  charged.

## The explorers

All three implement the `Explorer` trait (`new` + `run`) and follow the same
contract with the orchestrator: the orchestrator polls each explorer once per
cycle with a `BagContentRequest`, which is that explorer's turn. During the turn
an explorer may ask to move (`NeighborsRequest` then `TravelToPlanetRequest`)
before answering with its bag. Every wait uses a timeout, so a slow planet or
orchestrator never deadlocks an explorer.

**Miner (`MockExplorer`).** On arrival it queries the planet for its supported
basics and combinations and caches them. Each turn it mines one of the supported
basics (rotating through them) and crafts a `Diamond` when the planet supports it
and it holds two `Carbon`. Every few turns it travels to a neighbour. Because it
mines whatever the planet actually produces, it works on every planet type in the
galaxy, not just ours.

**AI (`AiExplorer`).** Goal-driven. It keeps per-planet knowledge and moves
through phases (exploring, collecting, crafting, done). It picks the most
valuable complex resource the galaxy can actually make from a wishlist
(`AIPartner`, `Dolphin`, `Robot`, `Diamond`), gathers the basic resources that
recipe needs across the reachable planets, then crafts. Planets it can't reach
and resources nobody produces are remembered and avoided.

**Smart (`SmartExplorer`).** A single-minded `Diamond` collector. Each turn it
decides one move: forge (fuse two `Carbon` into a `Diamond`) if a forge is
available here, mine a `Carbon` if it needs more, or wander if the planet can't
help. After collecting five `Diamond` it stops crafting and just roams.

## The orchestrator and game

The orchestrator owns the galaxy and every actor thread.

- **Planets** are driven with a request/ack pattern: send a message, block on the
  matching reply. A shared return channel is split per planet by a small
  demultiplexer.
- **Explorers** are polled as described above; the orchestrator also answers the
  travel requests they raise during their turn, running the incoming/outgoing
  handshake with the two planets involved.
- When a planet is hit with no rocket it is destroyed: the orchestrator kills its
  thread, drops it from the galaxy, and relocates any explorer that was on it.

The galaxy is fully connected, so an explorer can travel from any planet to any
other. The roster is one AstroParrot planet plus seven planets from other groups
(Rust-eze, One Million Crabs, Luna4, TRIP, Skycartel, The Compiler Strikes Back,
Immutable Cosmic Borrow), pulled in as git dependencies. They share the same
`common-game` interface; the differences in their creation functions are hidden
behind a small factory.

The macroquad front-end renders the galaxy and reacts to orchestrator events
(sunray received, asteroid deflected, planet destroyed, explorer moved).

### Controls

- `Space`: pause / resume.
- `M`: switch between Auto (the simulation sends sunrays and asteroids on its own)
  and Manual (you drive them).
- Click a planet: select it and show its stats (name, type, charged cells,
  rocket, neighbours, explorers present).
- In Manual mode, with a planet selected: `S` sends it a sunray, `A` sends it an
  asteroid.
- `R`: restart after a game over. `Esc`: quit.

## Building and running

```
cargo run              # start the galaxy simulation
cargo test             # run the tests
cargo build --no-default-features   # library only (planet + explorers, no game)
```

The `game` feature (default) pulls in `macroquad` and the seven external planet
crates. Disabling it leaves just the planet library and the explorer types.
