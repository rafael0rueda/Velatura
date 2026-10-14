//! Print a renderer's output for a bundled theme. Used to refresh the golden
//! files after a deliberate renderer change:
//!
//!   cargo run -p velatura-theme --example render -- kitty aetherius

use velatura_theme::{
    Resolved, bundled, firefox, ghostty, gtk, kitty, nvim, ptyxis, shell, sourceview, starship,
    tmux,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let themes = bundled::themes();
    let (Some(target), Some(theme)) = (
        args.first(),
        args.get(1)
            .and_then(|id| themes.iter().find(|t| &t.id == id)),
    ) else {
        eprintln!(
            "usage: render kitty|ghostty|gtk|shell|tmux|nvim|nvim-palette|starship|ptyxis|sourceview|firefox <bundled theme id>"
        );
        std::process::exit(2);
    };
    let Some(resolved) = Resolved::new(theme, None) else {
        std::process::exit(2);
    };
    match target.as_str() {
        "kitty" => print!("{}", kitty::render(&resolved)),
        "ghostty" => print!("{}", ghostty::render(&resolved)),
        "gtk" => print!("{}", gtk::render(&resolved)),
        "shell" => print!("{}", shell::render(&resolved)),
        "tmux" => print!("{}", tmux::render(&resolved)),
        "nvim" => print!("{}", nvim::render(&resolved)),
        "nvim-palette" => print!("{}", nvim::render_palette(&resolved)),
        "starship" => print!("{}", starship::render(&resolved)),
        "ptyxis" => print!("{}", ptyxis::render(&resolved)),
        "sourceview" => print!("{}", sourceview::render(&resolved)),
        "firefox" => print!("{}", firefox::render(&resolved)),
        _ => std::process::exit(2),
    }
}
