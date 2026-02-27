use anyhow::Result;
use clap::Parser;
use tracing::info;
use tracing_subscriber::EnvFilter;

use pterminal_core::Config;
use pterminal_ui::{App, SlintApp};

#[derive(Parser, Debug)]
#[command(name = "pterminal")]
#[command(about = "A GPU-accelerated terminal emulator")]
struct Args {
    /// Use raw winit backend instead of Slint
    #[arg(long)]
    raw: bool,
}

fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    info!("pterminal v{}", env!("CARGO_PKG_VERSION"));

    // Load config
    let config = Config::load().unwrap_or_else(|e| {
        tracing::warn!("Failed to load config: {}, using defaults", e);
        Config::default()
    });

    let args = Args::parse();

    if args.raw {
        // Use raw winit backend
        let app = App::new(config);
        app.run()
    } else {
        // Use Slint backend (default)
        let app = SlintApp::new(config);
        app.run()
    }
}
