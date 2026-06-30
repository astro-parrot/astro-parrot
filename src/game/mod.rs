//! The AstroParrot game: an animated galaxy on top of the real orchestrator.
//!
//! A fixed set of planets defends itself from asteroids. Explorers roam the
//! galaxy on their own (the orchestrator services their travel requests). The
//! user observes, can pause/resume, switch between an autonomous (Auto) and a
//! hands-on (Manual) mode, and click any planet to inspect it and — in Manual
//! mode — send it an individual sunray or asteroid.

mod orchestrator;

use std::collections::{HashMap, VecDeque};
use std::f32::consts::TAU;

use common_game::components::planet::DummyPlanetState;
use common_game::components::resource::{ComplexResourceType, ResourceType};
use common_game::utils::ID;
use macroquad::prelude::*;
use macroquad::rand::gen_range;

use orchestrator::{Command, GuiEvent, Orchestrator};

const PLANET_RADIUS: f32 = 46.0;
const ROCKET_SPEED: f32 = 480.0;
const SUNRAY_SPEED: f32 = 420.0;
const EXPLORER_SPEED: f32 = 240.0;
const EXPLOSION_DUR: f32 = 0.6;
const NUM_EXPLORERS: usize = 2;

/// Public entry point used by `main`.
pub async fn run() {
    let mut game = Game::new();
    loop {
        let dt = get_frame_time();
        game.update(dt);
        game.draw();

        if is_key_pressed(KeyCode::Escape) {
            break;
        }
        if matches!(game.phase, Phase::GameOver) && is_key_pressed(KeyCode::R) {
            game = Game::new();
        }
        next_frame().await;
    }
}

#[derive(PartialEq)]
enum Phase {
    Playing,
    GameOver,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Auto,
    Manual,
}

struct PlanetView {
    id: ID,
    pos: Vec2,
    body: Color,
    continents: Vec<(Vec2, f32)>,
    charged: usize,
    total_cells: usize,
    has_rocket: bool,
}

impl PlanetView {
    fn new(id: ID, pos: Vec2) -> Self {
        let body = Color::new(gen_range(0.12, 0.25), gen_range(0.4, 0.6), gen_range(0.4, 0.7), 1.0);
        let continents = (0..3)
            .map(|_| (vec2(gen_range(-0.4, 0.4), gen_range(-0.4, 0.4)), gen_range(0.2, 0.36)))
            .collect();
        Self { id, pos, body, continents, charged: 0, total_cells: 1, has_rocket: false }
    }
}

struct ExplorerView {
    id: ID,
    pos: Vec2,
    dest: Option<ID>,
}

struct Asteroid {
    id: u64,
    target: ID,
    pos: Vec2,
    vel: Vec2,
    radius: f32,
    rot: f32,
    rot_speed: f32,
    resolved: bool,
    dead: bool,
}

struct RocketShot {
    pos: Vec2,
    vel: Vec2,
    target_id: u64,
    ttl: f32,
    done: bool,
}

impl RocketShot {
    fn new(from: Vec2, target_id: u64) -> Self {
        Self { pos: from, vel: Vec2::ZERO, target_id, ttl: 4.0, done: false }
    }
}

struct SunrayParticle {
    target: ID,
    pos: Vec2,
    vel: Vec2,
    done: bool,
}

struct Explosion {
    pos: Vec2,
    t: f32,
    max_r: f32,
    color: Color,
}

impl Explosion {
    fn blast(pos: Vec2) -> Self {
        Self { pos, t: 0.0, max_r: 48.0, color: ORANGE }
    }
    fn spark(pos: Vec2) -> Self {
        Self { pos, t: 0.0, max_r: 18.0, color: GOLD }
    }
    fn big(pos: Vec2) -> Self {
        Self { pos, t: 0.0, max_r: PLANET_RADIUS * 2.6, color: ORANGE }
    }
}

struct Star {
    pos: Vec2,
    size: f32,
    phase: f32,
}

struct Game {
    orch: Orchestrator,
    phase: Phase,
    paused: bool,
    mode: Mode,
    selected: Option<ID>,
    selected_stats: Option<DummyPlanetState>,

    planets: Vec<PlanetView>,
    explorers: Vec<ExplorerView>,
    asteroids: Vec<Asteroid>,
    rockets: Vec<RocketShot>,
    sunrays: Vec<SunrayParticle>,
    explosions: Vec<Explosion>,
    stars: Vec<Star>,

    awaiting: HashMap<ID, VecDeque<u64>>,
    next_ast_id: u64,

    asteroid_interval: f32,
    asteroid_timer: f32,
    sunray_timer: f32,
    state_timer: f32,
    poll_timer: f32,
    stats_timer: f32,

    last_event: String,
}

impl Game {
    fn new() -> Self {
        let mut orch = Orchestrator::new();
        let (w, h) = (screen_width(), screen_height());
        let center = vec2(w * 0.5, h * 0.5);
        let radius = w.min(h) * 0.33;

        let stars = (0..150)
            .map(|_| Star {
                pos: vec2(gen_range(0.0, w), gen_range(0.0, h)),
                size: gen_range(0.5, 2.0),
                phase: gen_range(0.0, TAU),
            })
            .collect();

        let ids = orch.populate_galaxy().expect("failed to build the galaxy");
        let n = ids.len();
        let mut planets = Vec::new();
        for (i, &id) in ids.iter().enumerate() {
            let angle = i as f32 / n as f32 * TAU - std::f32::consts::FRAC_PI_2;
            let pos = center + vec2(angle.cos(), angle.sin()) * radius;
            planets.push(PlanetView::new(id, pos));
        }

        let mut explorers = Vec::new();
        for k in 0..NUM_EXPLORERS {
            let home = &planets[k * n / NUM_EXPLORERS];
            let id = orch.add_explorer(home.id).expect("failed to add explorer");
            explorers.push(ExplorerView { id, pos: home.pos, dest: None });
        }

        Self {
            orch,
            phase: Phase::Playing,
            paused: false,
            mode: Mode::Auto,
            selected: None,
            selected_stats: None,
            planets,
            explorers,
            asteroids: Vec::new(),
            rockets: Vec::new(),
            sunrays: Vec::new(),
            explosions: Vec::new(),
            stars,
            awaiting: HashMap::new(),
            next_ast_id: 0,
            asteroid_interval: 2.6,
            asteroid_timer: 2.6,
            sunray_timer: 0.5,
            state_timer: 0.2,
            poll_timer: 0.6,
            stats_timer: 0.0,
            last_event: "Auto mode. Space: pause · M: manual · click a planet for stats.".to_string(),
        }
    }

    // ----- update -------------------------------------------------------

    fn update(&mut self, dt: f32) {
        self.handle_input();
        if self.phase == Phase::Playing && !self.paused {
            self.update_playing(dt);
            self.consume_events();
            if self.orch.alive_planets().is_empty() {
                self.phase = Phase::GameOver;
                self.last_event = "All planets destroyed!".to_string();
            }
        }
        self.refresh_selected_stats(dt);
    }

    fn handle_input(&mut self) {
        if is_key_pressed(KeyCode::Space) {
            self.paused = !self.paused;
        }
        if is_key_pressed(KeyCode::M) {
            self.mode = match self.mode {
                Mode::Auto => Mode::Manual,
                Mode::Manual => Mode::Auto,
            };
            self.last_event = match self.mode {
                Mode::Auto => "Auto mode: the simulation runs itself.".to_string(),
                Mode::Manual => "Manual mode: select a planet, S = sunray, A = asteroid.".to_string(),
            };
        }
        if is_mouse_button_pressed(MouseButton::Left) {
            let m: Vec2 = mouse_position().into();
            self.selected = self.planet_at(m);
            self.selected_stats = None;
            self.stats_timer = 0.0;
        }

        if self.phase == Phase::Playing && !self.paused {
            if is_key_pressed(KeyCode::S)
                && let Some(p) = self.selected
            {
                self.spawn_sunray_to(p);
            }
            if is_key_pressed(KeyCode::A)
                && let Some(p) = self.selected
            {
                self.spawn_asteroid_to(p);
            }
        }
    }

    fn update_playing(&mut self, dt: f32) {
        self.refresh_states(dt);
        self.poll_explorers(dt);
        if self.mode == Mode::Auto {
            self.tick_auto_spawners(dt);
        }
        self.move_asteroids(dt);
        self.resolve_impacts();
        self.update_rockets(dt);
        self.update_sunrays(dt);
        self.update_explorers(dt);
        self.update_effects(dt);

        self.asteroids.retain(|a| !a.dead && on_screen(a.pos));
        self.rockets.retain(|r| !r.done && r.ttl > 0.0);
        self.sunrays.retain(|s| !s.done);
    }

    fn refresh_states(&mut self, dt: f32) {
        self.state_timer -= dt;
        if self.state_timer > 0.0 {
            return;
        }
        self.state_timer = 0.4;
        let ids: Vec<ID> = self.planets.iter().map(|p| p.id).collect();
        for id in ids {
            if let Some(state) = self.orch.planet_state(id)
                && let Some(pv) = self.planets.iter_mut().find(|p| p.id == id)
            {
                pv.charged = state.charged_cells_count;
                pv.total_cells = state.energy_cells.len();
                pv.has_rocket = state.has_rocket;
            }
        }
    }

    fn poll_explorers(&mut self, dt: f32) {
        self.poll_timer -= dt;
        if self.poll_timer > 0.0 {
            return;
        }
        self.poll_timer = 0.6;
        if let Err(e) = self.orch.poll_explorers() {
            self.last_event = e;
        }
    }

    fn tick_auto_spawners(&mut self, dt: f32) {
        self.sunray_timer -= dt;
        if self.sunray_timer <= 0.0 {
            let ids: Vec<ID> = self.planets.iter().map(|p| p.id).collect();
            for id in ids {
                self.spawn_sunray_to(id);
            }
            self.sunray_timer = 1.4;
        }
        self.asteroid_timer -= dt;
        if self.asteroid_timer <= 0.0 {
            if let Some(target) = self.random_planet() {
                self.spawn_asteroid_to(target);
            }
            self.asteroid_interval = (self.asteroid_interval - 0.05).max(0.9);
            self.asteroid_timer = self.asteroid_interval;
        }
    }

    fn move_asteroids(&mut self, dt: f32) {
        for a in &mut self.asteroids {
            if !a.dead {
                a.pos += a.vel * dt;
                a.rot += a.rot_speed * dt;
            }
        }
    }

    fn resolve_impacts(&mut self) {
        let mut i = 0;
        while i < self.asteroids.len() {
            if self.asteroids[i].dead || self.asteroids[i].resolved {
                i += 1;
                continue;
            }
            let target = self.asteroids[i].target;
            let aid = self.asteroids[i].id;
            let radius = self.asteroids[i].radius;
            let apos = self.asteroids[i].pos;

            let Some(pp) = self.planet_pos(target) else {
                self.asteroids[i].dead = true;
                i += 1;
                continue;
            };

            if apos.distance(pp) < PLANET_RADIUS + radius {
                self.asteroids[i].resolved = true;
                self.awaiting.entry(target).or_default().push_back(aid);
                let _ = self.orch.command(Command::SendAsteroid { planet: target });
            }
            i += 1;
        }
    }

    fn update_rockets(&mut self, dt: f32) {
        for r in &mut self.rockets {
            if r.done {
                continue;
            }
            let target = self.asteroids.iter().find(|a| a.id == r.target_id && !a.dead).map(|a| a.pos);
            let aim = target.unwrap_or(r.pos + r.vel);
            r.vel = (aim - r.pos).normalize_or_zero() * ROCKET_SPEED;
            r.pos += r.vel * dt;
            r.ttl -= dt;
        }
        for r in &mut self.rockets {
            if r.done {
                continue;
            }
            if let Some(a) = self.asteroids.iter_mut().find(|a| a.id == r.target_id && !a.dead)
                && r.pos.distance(a.pos) < a.radius + 7.0
            {
                a.dead = true;
                r.done = true;
                self.explosions.push(Explosion::blast(a.pos));
            }
        }
    }

    fn update_sunrays(&mut self, dt: f32) {
        let mut hits: Vec<ID> = Vec::new();
        for s in &mut self.sunrays {
            if s.done {
                continue;
            }
            if let Some(pp) = self.planets.iter().find(|p| p.id == s.target).map(|p| p.pos) {
                s.vel = (pp - s.pos).normalize_or_zero() * SUNRAY_SPEED;
                s.pos += s.vel * dt;
                if s.pos.distance(pp) < PLANET_RADIUS {
                    s.done = true;
                    hits.push(s.target);
                }
            } else {
                s.done = true;
            }
        }
        for planet in hits {
            let _ = self.orch.command(Command::SendSunray { planet });
        }
    }

    fn update_explorers(&mut self, dt: f32) {
        let t = get_time() as f32;
        for k in 0..self.explorers.len() {
            let id = self.explorers[k].id;
            let dest = self.explorers[k].dest.or_else(|| self.orch.explorer_planet(id));
            let Some(dest_id) = dest else { continue };
            let Some(dest_pos) = self.planet_pos(dest_id) else {
                self.explorers[k].dest = None;
                continue;
            };
            if self.explorers[k].dest.is_some() {
                let dir = (dest_pos - self.explorers[k].pos).normalize_or_zero();
                self.explorers[k].pos += dir * EXPLORER_SPEED * dt;
                if self.explorers[k].pos.distance(dest_pos) < PLANET_RADIUS + 22.0 {
                    self.explorers[k].dest = None;
                }
            } else {
                let phase = t * 1.1 + k as f32 * 2.0;
                self.explorers[k].pos = dest_pos + vec2(phase.cos(), phase.sin()) * (PLANET_RADIUS + 20.0);
            }
        }
    }

    fn update_effects(&mut self, dt: f32) {
        for e in &mut self.explosions {
            e.t += dt;
        }
        self.explosions.retain(|e| e.t < EXPLOSION_DUR);
    }

    fn consume_events(&mut self) {
        for event in self.orch.drain_events() {
            match event {
                GuiEvent::SunrayReceived { planet } => {
                    if let Some(pp) = self.planet_pos(planet) {
                        self.explosions.push(Explosion::spark(pp));
                    }
                }
                GuiEvent::AsteroidDeflected { planet } => {
                    if let Some(aid) = self.awaiting.get_mut(&planet).and_then(VecDeque::pop_front)
                        && let Some(pp) = self.planet_pos(planet)
                    {
                        self.rockets.push(RocketShot::new(pp, aid));
                    }
                    self.last_event = format!("Planet #{planet} deflected an asteroid! 🚀");
                }
                GuiEvent::PlanetDestroyed { planet } => {
                    let pos = self.planet_pos(planet);
                    for aid in self.awaiting.remove(&planet).unwrap_or_default() {
                        if let Some(a) = self.asteroids.iter_mut().find(|a| a.id == aid) {
                            a.dead = true;
                        }
                    }
                    if let Some(pp) = pos {
                        self.explosions.push(Explosion::big(pp));
                    }
                    self.planets.retain(|p| p.id != planet);
                    if self.selected == Some(planet) {
                        self.selected = None;
                        self.selected_stats = None;
                    }
                    self.last_event = format!("Planet #{planet} destroyed!");
                }
                GuiEvent::ExplorerMoved { explorer, to } => {
                    if let Some(ev) = self.explorers.iter_mut().find(|e| e.id == explorer) {
                        ev.dest = Some(to);
                    }
                }
            }
        }
    }

    fn refresh_selected_stats(&mut self, dt: f32) {
        let Some(p) = self.selected else {
            self.selected_stats = None;
            return;
        };
        self.stats_timer -= dt;
        if self.stats_timer > 0.0 {
            return;
        }
        self.stats_timer = 0.3;
        match self.orch.planet_state(p) {
            Some(state) => self.selected_stats = Some(state),
            None => {
                self.selected = None;
                self.selected_stats = None;
            }
        }
    }

    // ----- spawners -----------------------------------------------------

    fn spawn_sunray_to(&mut self, planet: ID) {
        let sun = self.sun_pos();
        self.sunrays.push(SunrayParticle { target: planet, pos: sun, vel: Vec2::ZERO, done: false });
    }

    fn spawn_asteroid_to(&mut self, target: ID) {
        let Some(tpos) = self.planet_pos(target) else {
            return;
        };
        let (w, h) = (screen_width(), screen_height());
        let pos = match gen_range(0, 4) {
            0 => vec2(gen_range(0.0, w), -30.0),
            1 => vec2(w + 30.0, gen_range(0.0, h)),
            2 => vec2(gen_range(0.0, w), h + 30.0),
            _ => vec2(-30.0, gen_range(0.0, h)),
        };
        let aim = tpos + vec2(gen_range(-22.0, 22.0), gen_range(-22.0, 22.0));
        let speed = gen_range(70.0, 120.0);
        self.asteroids.push(Asteroid {
            id: self.next_ast_id,
            target,
            pos,
            vel: (aim - pos).normalize_or_zero() * speed,
            radius: gen_range(15.0, 28.0),
            rot: gen_range(0.0, 360.0),
            rot_speed: gen_range(-90.0, 90.0),
            resolved: false,
            dead: false,
        });
        self.next_ast_id += 1;
    }

    // ----- lookups ------------------------------------------------------

    fn sun_pos(&self) -> Vec2 {
        vec2(screen_width() - 70.0, 70.0)
    }

    fn planet_pos(&self, id: ID) -> Option<Vec2> {
        self.planets.iter().find(|p| p.id == id).map(|p| p.pos)
    }

    fn planet_at(&self, m: Vec2) -> Option<ID> {
        self.planets
            .iter()
            .find(|p| m.distance(p.pos) < PLANET_RADIUS + 6.0)
            .map(|p| p.id)
    }

    fn random_planet(&self) -> Option<ID> {
        if self.planets.is_empty() {
            return None;
        }
        Some(self.planets[gen_range(0, self.planets.len())].id)
    }

    // ----- drawing ------------------------------------------------------

    fn draw(&self) {
        clear_background(Color::new(0.02, 0.02, 0.06, 1.0));
        self.draw_stars();
        self.draw_sun();
        for s in &self.sunrays {
            draw_sunray(s);
        }
        for p in &self.planets {
            draw_planet(p, self.selected == Some(p.id));
        }
        for a in &self.asteroids {
            draw_asteroid(a);
        }
        for r in &self.rockets {
            draw_rocket(r);
        }
        for e in &self.explosions {
            draw_explosion(e);
        }
        for ev in &self.explorers {
            self.draw_explorer(ev);
        }
        self.draw_hud();
        if self.selected.is_some() {
            self.draw_stats();
        }
        if self.phase == Phase::GameOver {
            self.draw_game_over();
        }
    }

    fn draw_stars(&self) {
        let t = get_time() as f32;
        for s in &self.stars {
            let a = 0.4 + 0.6 * (0.5 + 0.5 * (t * 1.5 + s.phase).sin());
            draw_circle(s.pos.x, s.pos.y, s.size, Color::new(1.0, 1.0, 1.0, a * 0.8));
        }
    }

    fn draw_sun(&self) {
        let sun = self.sun_pos();
        let t = get_time() as f32;
        for i in 0..12 {
            let ang = t * 0.3 + i as f32 * TAU / 12.0;
            let len = 30.0 + 5.0 * (t * 3.0 + i as f32).sin();
            let dir = vec2(ang.cos(), ang.sin());
            draw_line(
                sun.x + dir.x * 26.0,
                sun.y + dir.y * 26.0,
                sun.x + dir.x * (26.0 + len),
                sun.y + dir.y * (26.0 + len),
                3.0,
                Color::new(1.0, 0.85, 0.3, 0.7),
            );
        }
        draw_circle(sun.x, sun.y, 30.0, GOLD);
        draw_circle(sun.x, sun.y, 22.0, YELLOW);
        draw_circle(sun.x - 6.0, sun.y - 6.0, 10.0, Color::new(1.0, 1.0, 0.85, 0.9));
    }

    fn draw_explorer(&self, ev: &ExplorerView) {
        let pos = ev.pos;
        if let Some(dest) = ev.dest
            && let Some(pp) = self.planet_pos(dest)
        {
            draw_line(pos.x, pos.y, pp.x, pp.y, 1.0, Color::new(0.4, 0.9, 1.0, 0.25));
        }
        draw_circle(pos.x, pos.y, 12.0, Color::new(0.3, 0.9, 1.0, 0.15));
        let dir = ev
            .dest
            .and_then(|id| self.planet_pos(id))
            .map_or(vec2(0.0, -1.0), |pp| (pp - pos).normalize_or_zero());
        draw_dir_triangle(pos, dir, 9.0, Color::new(0.5, 0.95, 1.0, 1.0));
        draw_circle(pos.x, pos.y, 3.0, WHITE);
    }

    fn draw_hud(&self) {
        draw_panel(12.0, 12.0, 250.0, 96.0);
        draw_text("AstroParrot", 24.0, 40.0, 28.0, WHITE);
        let mode = if self.mode == Mode::Auto { "AUTO" } else { "MANUAL" };
        let mode_color = if self.mode == Mode::Auto { SKYBLUE } else { ORANGE };
        draw_text(format!("Mode: {mode}"), 24.0, 94.0, 22.0, mode_color);
        if self.paused {
            draw_text("PAUSED", 150.0, 94.0, 22.0, RED);
        }

        draw_text(&self.last_event, 24.0, 130.0, 20.0, Color::new(1.0, 1.0, 1.0, 0.85));

        // Explorer roster (bottom-left).
        let n = self.explorers.len();
        let y0 = screen_height() - 28.0 - n as f32 * 22.0;
        draw_panel(12.0, y0 - 24.0, 300.0, 24.0 + n as f32 * 22.0 + 8.0);
        draw_text("Explorers", 24.0, y0 - 6.0, 20.0, WHITE);
        for (i, ev) in self.explorers.iter().enumerate() {
            let here = self.orch.explorer_planet(ev.id).map_or("—".to_string(), |p| format!("#{p}"));
            let (basics, diamonds) = self.bag_counts(ev.id);
            draw_text(
                format!("#{} @ {here}   res:{basics}  💎:{diamonds}", ev.id),
                24.0,
                y0 + 18.0 + i as f32 * 22.0,
                18.0,
                Color::new(0.8, 0.9, 1.0, 1.0),
            );
        }

        let hint = "Space pause · M mode · click planet: stats · (Manual) S sunray  A asteroid · R restart · ESC quit";
        let w = measure_text(hint, None, 18, 1.0).width;
        draw_text(hint, screen_width() - w - 24.0, screen_height() - 16.0, 18.0, GRAY);
    }

    fn draw_stats(&self) {
        let Some(id) = self.selected else { return };
        let x = screen_width() - 282.0;
        let y = 12.0;
        draw_panel(x, y, 270.0, 174.0);
        let name = self.orch.planet_name(id).unwrap_or("?");
        let ptype = self.orch.planet_type(id).unwrap_or("?");
        draw_text(format!("#{id} · {name}"), x + 12.0, y + 26.0, 22.0, WHITE);
        draw_text(format!("Type {ptype} planet"), x + 12.0, y + 48.0, 18.0, GOLD);
        if let Some(state) = &self.selected_stats {
            draw_text(
                format!("Charged cells: {}/{}", state.charged_cells_count, state.energy_cells.len()),
                x + 12.0,
                y + 74.0,
                20.0,
                SKYBLUE,
            );
            let rocket = if state.has_rocket { "ready" } else { "none" };
            draw_text(format!("Rocket: {rocket}"), x + 12.0, y + 96.0, 20.0, SKYBLUE);
        } else {
            draw_text("querying...", x + 12.0, y + 74.0, 20.0, GRAY);
        }
        let mut neighbours = self.orch.neighbours(id);
        neighbours.sort_unstable();
        draw_text(
            format!("Neighbours: {neighbours:?}"),
            x + 12.0,
            y + 124.0,
            18.0,
            Color::new(0.8, 0.9, 1.0, 1.0),
        );
        let here = self.orch.explorers_on(id);
        draw_text(format!("Explorers here: {here:?}"), x + 12.0, y + 148.0, 18.0, Color::new(0.8, 0.9, 1.0, 1.0));
    }

    fn draw_game_over(&self) {
        draw_rectangle(0.0, 0.0, screen_width(), screen_height(), Color::new(0.0, 0.0, 0.0, 0.6));
        center_text("GALAXY LOST", screen_height() * 0.5 - 30.0, 52.0, RED);
        center_text("Press R to restart", screen_height() * 0.5 + 56.0, 26.0, Color::new(1.0, 1.0, 1.0, 0.8));
    }

    /// Returns `(total basic resources, diamonds)` in the explorer's bag.
    fn bag_counts(&self, explorer: ID) -> (usize, usize) {
        let Some(bag) = self.orch.bag(explorer) else {
            return (0, 0);
        };
        let basics: usize = bag
            .content
            .iter()
            .filter(|(k, _)| matches!(k, ResourceType::Basic(_)))
            .map(|(_, v)| *v)
            .sum();
        let diamonds = bag
            .content
            .get(&ResourceType::Complex(ComplexResourceType::Diamond))
            .copied()
            .unwrap_or(0);
        (basics, diamonds)
    }
}

// ----- free drawing helpers --------------------------------------------

fn on_screen(p: Vec2) -> bool {
    p.x > -120.0 && p.x < screen_width() + 120.0 && p.y > -120.0 && p.y < screen_height() + 120.0
}

fn draw_dir_triangle(pos: Vec2, dir: Vec2, size: f32, color: Color) {
    let d = if dir.length() < 0.001 { vec2(0.0, -1.0) } else { dir.normalize() };
    let perp = vec2(-d.y, d.x);
    let nose = pos + d * size;
    let left = pos - d * size * 0.6 + perp * size * 0.6;
    let right = pos - d * size * 0.6 - perp * size * 0.6;
    draw_triangle(nose, left, right, color);
}

fn draw_planet(p: &PlanetView, selected: bool) {
    let r = PLANET_RADIUS;
    let c = p.pos;

    if selected {
        draw_circle_lines(c.x, c.y, r + 14.0, 2.0, Color::new(1.0, 1.0, 0.5, 0.8));
    }

    let glow = if p.charged > 0 { 0.16 } else { 0.07 };
    draw_circle(c.x, c.y, r + 10.0, Color::new(0.3, 0.9, 0.6, glow));
    draw_circle_lines(c.x, c.y, r + 6.0, 2.0, Color::new(0.4, 1.0, 0.7, 0.25));

    draw_circle(c.x, c.y, r, p.body);
    let land = Color::new(0.18, 0.55, 0.34, 1.0);
    for (off, rf) in &p.continents {
        draw_circle(c.x + off.x * r, c.y + off.y * r, rf * r, land);
    }
    draw_circle(c.x - r * 0.3, c.y - r * 0.3, r * 0.4, Color::new(1.0, 1.0, 1.0, 0.10));
    draw_circle(c.x + r * 0.34, c.y + r * 0.2, r * 0.95, Color::new(0.0, 0.0, 0.0, 0.22));

    if p.has_rocket {
        let base = vec2(c.x, c.y - r - 2.0);
        draw_dir_triangle(base + vec2(0.0, -5.0), vec2(0.0, -1.0), 10.0, LIGHTGRAY);
        draw_dir_triangle(base + vec2(0.0, -8.0), vec2(0.0, -1.0), 5.0, RED);
        draw_dir_triangle(base + vec2(0.0, 5.0), vec2(0.0, 1.0), 6.0, ORANGE);
    }

    let cells = p.total_cells.max(1);
    for i in 0..cells {
        let x = c.x - (cells as f32 - 1.0) * 8.0 + i as f32 * 16.0;
        let y = c.y + r + 14.0;
        let fill = if i < p.charged {
            Color::new(1.0, 0.85, 0.2, 1.0)
        } else {
            Color::new(0.2, 0.2, 0.25, 1.0)
        };
        draw_rectangle(x - 5.0, y - 7.0, 10.0, 14.0, fill);
        draw_rectangle_lines(x - 5.0, y - 7.0, 10.0, 14.0, 1.5, LIGHTGRAY);
    }

    let label = format!("#{}", p.id);
    let w = measure_text(&label, None, 18, 1.0).width;
    draw_text(&label, c.x - w * 0.5, c.y + r + 38.0, 18.0, Color::new(0.8, 0.9, 1.0, 0.9));
}

fn draw_asteroid(a: &Asteroid) {
    let tint = if a.resolved {
        Color::new(0.6, 0.35, 0.3, 1.0)
    } else {
        Color::new(0.45, 0.38, 0.34, 1.0)
    };
    draw_poly(a.pos.x, a.pos.y, 7, a.radius, a.rot, tint);
    draw_poly_lines(a.pos.x, a.pos.y, 7, a.radius, a.rot, 2.0, Color::new(0.25, 0.2, 0.18, 1.0));
    let crater = Color::new(0.3, 0.25, 0.22, 1.0);
    draw_circle(a.pos.x - a.radius * 0.3, a.pos.y - a.radius * 0.2, a.radius * 0.22, crater);
    draw_circle(a.pos.x + a.radius * 0.25, a.pos.y + a.radius * 0.25, a.radius * 0.16, crater);
}

fn draw_rocket(r: &RocketShot) {
    let d = if r.vel.length() < 0.01 { vec2(0.0, -1.0) } else { r.vel.normalize() };
    let flame_len = 8.0 + 4.0 * ((get_time() as f32 * 30.0).sin()).abs();
    draw_dir_triangle(r.pos - d * 10.0, -d, flame_len, ORANGE);
    draw_dir_triangle(r.pos, d, 11.0, LIGHTGRAY);
    draw_dir_triangle(r.pos + d * 4.0, d, 6.0, RED);
}

fn draw_sunray(s: &SunrayParticle) {
    let tail = s.pos - s.vel.normalize_or_zero() * 14.0;
    draw_line(tail.x, tail.y, s.pos.x, s.pos.y, 3.0, Color::new(1.0, 0.9, 0.4, 0.8));
    draw_circle(s.pos.x, s.pos.y, 3.5, Color::new(1.0, 0.95, 0.6, 0.95));
}

fn draw_explosion(e: &Explosion) {
    let p = (e.t / EXPLOSION_DUR).clamp(0.0, 1.0);
    let r = e.max_r * p;
    let alpha = 1.0 - p;
    draw_circle(e.pos.x, e.pos.y, r, Color::new(e.color.r, e.color.g, e.color.b, alpha * 0.5));
    draw_circle_lines(e.pos.x, e.pos.y, r, 3.0, Color::new(1.0, 0.85, 0.4, alpha));
    for i in 0..6 {
        let ang = i as f32 * TAU / 6.0;
        let d = vec2(ang.cos(), ang.sin());
        let sp = e.pos + d * r;
        draw_circle(sp.x, sp.y, 2.5 * alpha + 0.5, Color::new(1.0, 0.7, 0.2, alpha));
    }
}

fn draw_panel(x: f32, y: f32, w: f32, h: f32) {
    draw_rectangle(x, y, w, h, Color::new(0.05, 0.07, 0.12, 0.7));
    draw_rectangle_lines(x, y, w, h, 2.0, Color::new(0.3, 0.4, 0.55, 0.6));
}

fn center_text(text: &str, y: f32, size: f32, color: Color) {
    let w = measure_text(text, None, size as u16, 1.0).width;
    draw_text(text, (screen_width() - w) * 0.5, y, size, color);
}
