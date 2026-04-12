mod config;
mod telemetry;

use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::{info, error};

fn main() {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    info!("home-ems v{}", env!("CARGO_PKG_VERSION"));

    // Parse CLI args
    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config.yaml".to_string());

    // Load config
    let config = match config::Config::load(Path::new(&config_path)) {
        Ok(c) => c,
        Err(e) => {
            error!("failed to load config '{}': {}", config_path, e);
            std::process::exit(1);
        }
    };

    info!("site: {}", config.site.name);
    info!("fuse limit: {}A / {} phases (max {}W)",
        config.fuse.max_amps, config.fuse.phases, config.fuse.max_power_w());
    info!("control interval: {}s, grid target: {}W",
        config.site.control_interval_s, config.site.grid_target_w);

    for driver in &config.drivers {
        info!("driver: {} (lua: {}, site_meter: {}, battery: {} Wh)",
            driver.name, driver.lua, driver.is_site_meter, driver.battery_capacity_wh);
    }

    // Initialize telemetry store
    let _store = Arc::new(Mutex::new(
        telemetry::TelemetryStore::new(config.site.smoothing_alpha)
    ));

    // Graceful shutdown
    let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        info!("shutdown signal received");
        r.store(false, std::sync::atomic::Ordering::SeqCst);
    }).expect("failed to set ctrl-c handler");

    // TODO: Start driver threads (Lua runtime + host API)
    // TODO: Start control loop thread
    // TODO: Start REST API thread (tiny_http)
    // TODO: Start HA MQTT bridge thread

    info!("home-ems running - press Ctrl+C to stop");

    while running.load(std::sync::atomic::Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    info!("home-ems stopped");
}
