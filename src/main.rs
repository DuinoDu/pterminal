use anyhow::Result;
use tracing::info;
use tracing_subscriber::EnvFilter;

use pterminal_core::Config;
use pterminal_ui::SlintApp;

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

    // Run the Slint-based application
    let app = SlintApp::new(config);
    app.run()
}
