mod config;
mod content;
mod daemon;
mod picker;
mod storage;
mod sync;
mod theme;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "copyninja",
    about = "Clipboard history manager for Linux",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the clipboard monitoring daemon
    Daemon,
    /// Open the clipboard picker UI
    Pick,
    /// List the built-in picker themes
    Themes,
}

fn main() {
    let (config, config_warnings) = config::load();

    // Initialize logging from config (RUST_LOG env var overrides)
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(&config.log_level))
        .init();
    // Reported only now: the logger didn't exist while the config was read.
    for warning in &config_warnings {
        log::warn!("Config: {}", warning);
    }

    let cli = Cli::parse();

    match cli.command {
        Commands::Daemon => {
            storage::init(&config);
            daemon::run();
        }
        Commands::Pick => {
            storage::init(&config);
            picker::run(&config);
        }
        Commands::Themes => list_themes(&config.appearance.theme),
    }
}

fn list_themes(current: &str) {
    println!("Built-in themes (set with `theme = \"<name>\"` under [appearance]):\n");
    for (name, dark, _) in theme::PRESETS {
        let marker = if *name == current { "  (current)" } else { "" };
        let kind = if *dark { "dark" } else { "light" };
        println!("  {:<22} {}{}", name, kind, marker);
    }
    let auto = if current == "auto" { "  (current)" } else { "" };
    println!(
        "  {:<22} follows the system's light/dark setting{}",
        "auto", auto
    );
}
