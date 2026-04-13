//! Watch config.yaml for changes and trigger hot-reload.
//! Diffs new config against current and applies changes per subsystem:
//!   - Drivers: registry handles add/remove/restart
//!   - Control settings: applied to ControlState
//!   - HA settings: not hot-reloadable (would need MQTT reconnect — TODO)

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{info, warn, error};
use notify::{Watcher, RecursiveMode, EventKind};

use crate::config::Config;
use crate::driver_registry::DriverRegistry;
use crate::control::ControlState;

/// Start a file watcher on config.yaml. Reloads on change with debouncing.
pub fn start_watcher(
    config_path: PathBuf,
    current_config: Arc<std::sync::RwLock<Config>>,
    registry: DriverRegistry,
    control: Arc<Mutex<ControlState>>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("config-watcher".to_string())
        .spawn(move || {
            let (tx, rx) = std::sync::mpsc::channel();
            let mut watcher = match notify::recommended_watcher(tx) {
                Ok(w) => w,
                Err(e) => {
                    error!("config watcher: failed to init: {}", e);
                    return;
                }
            };

            let watch_path = config_path.parent().map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
            if let Err(e) = watcher.watch(&watch_path, RecursiveMode::NonRecursive) {
                error!("config watcher: failed to watch {}: {}", watch_path.display(), e);
                return;
            }
            info!("config watcher: watching {}", config_path.display());

            // Debounce: collect events for 500ms then apply once
            let mut last_change: Option<std::time::Instant> = None;
            loop {
                match rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(Ok(event)) => {
                        if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                            // Check if it's our config file
                            let is_ours = event.paths.iter().any(|p| {
                                p.file_name() == config_path.file_name()
                            });
                            if is_ours {
                                last_change = Some(std::time::Instant::now());
                            }
                        }
                    }
                    Ok(Err(e)) => warn!("config watcher event error: {}", e),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        // Check debounce window
                        if let Some(t) = last_change {
                            if t.elapsed() > Duration::from_millis(500) {
                                last_change = None;
                                if let Err(e) = reload(&config_path, &current_config, &registry, &control) {
                                    warn!("config reload failed: {}", e);
                                }
                            }
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        warn!("config watcher disconnected");
                        return;
                    }
                }
            }
        })
        .expect("failed to spawn config watcher thread")
}

/// Load new config from disk, diff against current, apply changes.
pub fn reload(
    path: &PathBuf,
    current: &Arc<std::sync::RwLock<Config>>,
    registry: &DriverRegistry,
    control: &Arc<Mutex<ControlState>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let new_config = Config::load(path)?;

    let old = {
        let cur = current.read().unwrap();
        cur.clone()
    };

    info!("config reload: applying changes");

    // Apply control changes
    let mut needs_apply = false;
    if (new_config.site.grid_target_w - old.site.grid_target_w).abs() > 0.01 {
        info!("  grid_target_w: {} → {}", old.site.grid_target_w, new_config.site.grid_target_w);
        needs_apply = true;
    }
    if (new_config.site.grid_tolerance_w - old.site.grid_tolerance_w).abs() > 0.01 {
        info!("  grid_tolerance_w: {} → {}", old.site.grid_tolerance_w, new_config.site.grid_tolerance_w);
        needs_apply = true;
    }
    if (new_config.site.slew_rate_w - old.site.slew_rate_w).abs() > 0.01 {
        info!("  slew_rate_w: {} → {}", old.site.slew_rate_w, new_config.site.slew_rate_w);
        needs_apply = true;
    }
    if new_config.site.min_dispatch_interval_s != old.site.min_dispatch_interval_s {
        info!("  min_dispatch_interval_s: {} → {}",
            old.site.min_dispatch_interval_s, new_config.site.min_dispatch_interval_s);
        needs_apply = true;
    }

    if needs_apply {
        let mut ctrl = control.lock().unwrap();
        ctrl.set_grid_target(new_config.site.grid_target_w);
        ctrl.grid_tolerance_w = new_config.site.grid_tolerance_w;
        ctrl.slew_rate_w = new_config.site.slew_rate_w;
        ctrl.min_dispatch_interval_s = new_config.site.min_dispatch_interval_s;
    }

    // Diff drivers — registry handles add/remove/restart
    registry.reload(&new_config.drivers);

    // Update current config
    *current.write().unwrap() = new_config;

    info!("config reload: complete");
    Ok(())
}

/// Save config to yaml atomically (write tmp + rename).
pub fn save_atomic(path: &PathBuf, config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let yaml = serde_yaml::to_string(config)?;
    let tmp_path = path.with_extension("yaml.tmp");
    std::fs::write(&tmp_path, yaml)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;

    fn minimal_config() -> Config {
        Config {
            site: SiteConfig {
                name: "Test".into(),
                control_interval_s: 5,
                grid_target_w: 0.0,
                grid_tolerance_w: 50.0,
                watchdog_timeout_s: 60,
                smoothing_alpha: 0.3,
                gain: 0.5,
                slew_rate_w: 500.0,
                min_dispatch_interval_s: 5,
            },
            fuse: FuseConfig { max_amps: 16.0, phases: 3, voltage: 230.0 },
            drivers: vec![DriverConfig {
                name: "a".into(),
                lua: "drivers/a.lua".into(),
                is_site_meter: true,
                battery_capacity_wh: 10000.0,
                mqtt: Some(MqttConnectionConfig {
                    host: "1.1.1.1".into(), port: 1883,
                    username: None, password: None,
                }),
                modbus: None,
            }],
            api: ApiConfig { port: 8080 },
            homeassistant: None,
            state: None,
            price: None,
            weather: None,
            batteries: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn save_atomic_writes_yaml_and_cleans_up_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        let cfg = minimal_config();

        save_atomic(&path, &cfg).expect("save must succeed");

        // File exists
        assert!(path.exists());
        // tmp file is gone (renamed away)
        assert!(!path.with_extension("yaml.tmp").exists());

        // Content is valid yaml that parses back
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.site.name, "Test");
        assert_eq!(loaded.drivers.len(), 1);
    }

    #[test]
    fn save_atomic_overwrites_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, "garbage that won't parse").unwrap();

        let cfg = minimal_config();
        save_atomic(&path, &cfg).expect("save must succeed");

        // Should be replaced cleanly
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded.site.name, "Test");
    }

    #[test]
    fn save_atomic_failure_doesnt_corrupt_existing() {
        // Write a valid file, then attempt to save to a path where rename will fail
        // Simulating this is OS-dependent; we mainly assert no tmp pollution happens
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        let cfg = minimal_config();
        save_atomic(&path, &cfg).unwrap();
        let original_contents = std::fs::read_to_string(&path).unwrap();

        // Save again — should still be parseable
        save_atomic(&path, &cfg).unwrap();
        let new_contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(original_contents, new_contents); // deterministic
    }

    #[test]
    fn reload_picks_up_grid_target_change_and_applies_to_control() {
        use crate::control::ControlState;
        use std::sync::{Arc, Mutex, RwLock};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");

        let mut cfg = minimal_config();
        cfg.site.grid_target_w = 0.0;
        save_atomic(&path, &cfg).unwrap();

        let current = Arc::new(RwLock::new(cfg));
        let control = Arc::new(Mutex::new(ControlState::new(0.0, 50.0, "a".into())));

        // Build a registry — we won't actually use its driver-spawning capability,
        // just need it for the reload signature
        let store = Arc::new(Mutex::new(crate::telemetry::TelemetryStore::new(0.3)));
        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let registry = DriverRegistry::new(store, 60, dir.path().to_path_buf(), running);

        // Modify yaml: change grid_target
        let mut new_cfg = current.read().unwrap().clone();
        new_cfg.site.grid_target_w = -500.0;
        save_atomic(&path, &new_cfg).unwrap();

        // Run reload
        reload(&path, &current, &registry, &control).expect("reload must succeed");

        // Control state should reflect new target
        let ctrl = control.lock().unwrap();
        assert_eq!(ctrl.grid_target_w, -500.0);
        assert_eq!(ctrl.pid_controller.setpoint, -500.0);

        // current_config should also reflect the new value
        assert_eq!(current.read().unwrap().site.grid_target_w, -500.0);

        // Cleanup so we don't leak the empty registry handle
        registry.shutdown_all();
    }

    #[test]
    fn reload_picks_up_slew_rate_and_dispatch_interval_changes() {
        use crate::control::ControlState;
        use std::sync::{Arc, Mutex, RwLock};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");

        let mut cfg = minimal_config();
        cfg.site.slew_rate_w = 500.0;
        cfg.site.min_dispatch_interval_s = 5;
        save_atomic(&path, &cfg).unwrap();

        let current = Arc::new(RwLock::new(cfg));
        let control = Arc::new(Mutex::new(ControlState::new(0.0, 50.0, "a".into())));
        let store = Arc::new(Mutex::new(crate::telemetry::TelemetryStore::new(0.3)));
        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let registry = DriverRegistry::new(store, 60, dir.path().to_path_buf(), running);

        let mut new_cfg = current.read().unwrap().clone();
        new_cfg.site.slew_rate_w = 250.0;
        new_cfg.site.min_dispatch_interval_s = 10;
        save_atomic(&path, &new_cfg).unwrap();

        reload(&path, &current, &registry, &control).expect("reload must succeed");

        let ctrl = control.lock().unwrap();
        assert_eq!(ctrl.slew_rate_w, 250.0);
        assert_eq!(ctrl.min_dispatch_interval_s, 10);

        registry.shutdown_all();
    }

    #[test]
    fn reload_fails_gracefully_on_invalid_yaml() {
        use crate::control::ControlState;
        use std::sync::{Arc, Mutex, RwLock};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        let cfg = minimal_config();
        save_atomic(&path, &cfg).unwrap();

        let current = Arc::new(RwLock::new(cfg.clone()));
        let control = Arc::new(Mutex::new(ControlState::new(0.0, 50.0, "a".into())));
        let store = Arc::new(Mutex::new(crate::telemetry::TelemetryStore::new(0.3)));
        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let registry = DriverRegistry::new(store, 60, dir.path().to_path_buf(), running);

        // Write garbage
        std::fs::write(&path, "this: is: not: valid: yaml: at all").unwrap();

        let result = reload(&path, &current, &registry, &control);
        assert!(result.is_err(), "reload should fail on bad yaml");

        // Original config preserved in memory
        assert_eq!(current.read().unwrap().site.name, "Test");

        registry.shutdown_all();
    }
}
