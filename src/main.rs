#[cfg(feature = "game")]
mod game;

#[cfg(feature = "game")]
fn window_conf() -> macroquad::window::Conf {
    macroquad::window::Conf {
        window_title: "AstroParrot".to_owned(),
        window_width: 1000,
        window_height: 700,
        high_dpi: true,
        ..Default::default()
    }
}

#[cfg(feature = "game")]
#[macroquad::main(window_conf)]
async fn main() {
    game::run().await;
}

#[cfg(not(feature = "game"))]
fn main() {
    eprintln!("astro-parrot was built without the `game` feature; nothing to run.");
}
