//! The `velatura` command.
//!
//! Phase 0 skeleton: enough to prove the workspace links and to measure the
//! binary. The real command set arrives in phase 1a.

use std::process::ExitCode;

use velatura_engine::host::Dirs;
use velatura_theme::{Color, nearest_accent};

const USAGE: &str = "\
velatura (phase 0 skeleton)

Usage:
  velatura --version
  velatura accent <#RRGGBB>   show the nearest GNOME accent color
  velatura dirs               show the directories velatura would use
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--version"] | ["-V"] => {
            println!("velatura {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        ["accent", hex] => match Color::parse(hex) {
            Ok(color) => {
                let accent = nearest_accent(color);
                println!(
                    "{} -> {} ({})",
                    color.hex(),
                    accent.setting_value(),
                    accent.color().hex()
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("velatura: {e}");
                ExitCode::from(2)
            }
        },
        ["dirs"] => match Dirs::from_env(|k| std::env::var_os(k)) {
            Some(d) => {
                println!("config  {}", d.config.join("velatura").display());
                println!("data    {}", d.data.join("velatura").display());
                println!("state   {}", d.state.join("velatura").display());
                println!("cache   {}", d.cache.join("velatura").display());
                ExitCode::SUCCESS
            }
            None => {
                eprintln!("velatura: HOME is not set to an absolute path");
                ExitCode::from(2)
            }
        },
        _ => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
