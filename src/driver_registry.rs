//! Dynamic driver registry — spawn, stop, and reload drivers at runtime.
//!
//! Each driver runs in its own thread with its own Lua runtime and MQTT/Modbus
//! connections. The registry manages the lifecycle: adding new drivers, stopping
//! old ones, and reloading when config changes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;
use tracing::{info, warn, error};

use crate::config::DriverConfig;
use crate::telemetry::TelemetryStore;
use crate::lua;

/// Command channel messages sent from the control loop to each driver thread
#[derive(Debug)]
pub enum DriverCommand {
    Battery { power_w: f64 },
    DefaultMode,
    Shutdown,
}

/// Handle to a running driver — keep this to send commands or shut it down.
pub struct DriverHandle {
    pub name: String,
    pub config: DriverConfig,
    pub cmd_tx: mpsc::Sender<DriverCommand>,
    pub join: Option<JoinHandle<()>>,
}

impl DriverHandle {
    /// Signal the driver to shut down and wait for the thread to exit.
    pub fn shutdown(mut self, timeout: Duration) {
        let _ = self.cmd_tx.send(DriverCommand::DefaultMode);
        let _ = self.cmd_tx.send(DriverCommand::Shutdown);
        if let Some(handle) = self.join.take() {
            // Spawn a watchdog thread that joins with timeout
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = handle.join();
                let _ = tx.send(());
            });
            match rx.recv_timeout(timeout) {
                Ok(_) => info!("driver '{}' stopped cleanly", self.name),
                Err(_) => warn!("driver '{}' did not stop within {:?}, leaking thread", self.name, timeout),
            }
        }
    }
}

/// Registry of running drivers. Thread-safe via internal Mutex.
#[derive(Clone)]
pub struct DriverRegistry {
    inner: Arc<Mutex<HashMap<String, DriverHandle>>>,
    store: Arc<Mutex<TelemetryStore>>,
    watchdog_timeout_s: u64,
    lua_dir: PathBuf,
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl DriverRegistry {
    pub fn new(
        store: Arc<Mutex<TelemetryStore>>,
        watchdog_timeout_s: u64,
        lua_dir: PathBuf,
        running: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            store,
            watchdog_timeout_s,
            lua_dir,
            running,
        }
    }

    /// Spawn a new driver thread.
    pub fn add(&self, config: DriverConfig) {
        let name = config.name.clone();

        // Don't double-add
        {
            let inner = self.inner.lock().unwrap();
            if inner.contains_key(&name) {
                warn!("driver '{}' already registered, skipping add", name);
                return;
            }
        }

        let (tx, rx) = mpsc::channel::<DriverCommand>();
        let dc = config.clone();
        let store = self.store.clone();
        let watchdog_s = self.watchdog_timeout_s;
        let lua_dir = self.lua_dir.clone();
        let running = self.running.clone();

        let handle = std::thread::Builder::new()
            .name(format!("driver-{}", name))
            .spawn(move || {
                run_driver_thread(dc, store, watchdog_s, lua_dir, rx, running);
            })
            .expect("failed to spawn driver thread");

        let driver_handle = DriverHandle {
            name: name.clone(),
            config,
            cmd_tx: tx,
            join: Some(handle),
        };

        self.inner.lock().unwrap().insert(name.clone(), driver_handle);
        info!("driver '{}' added", name);
    }

    /// Stop and remove a driver.
    pub fn remove(&self, name: &str) {
        let driver = self.inner.lock().unwrap().remove(name);
        if let Some(d) = driver {
            info!("driver '{}' removing", name);
            d.shutdown(Duration::from_secs(10));
        }
    }

    /// Reload drivers based on new config. Diffs against current state:
    /// - New drivers → add
    /// - Missing drivers → remove
    /// - Changed config → remove + add
    pub fn reload(&self, new_configs: &[DriverConfig]) {
        // Snapshot current state
        let current: HashMap<String, DriverConfig> = {
            let inner = self.inner.lock().unwrap();
            inner.iter().map(|(k, v)| (k.clone(), v.config.clone())).collect()
        };

        let new_map: HashMap<String, DriverConfig> = new_configs.iter()
            .map(|c| (c.name.clone(), c.clone()))
            .collect();

        // Remove drivers no longer in config, or whose config changed
        for (name, old_cfg) in &current {
            match new_map.get(name) {
                None => self.remove(name),
                Some(new_cfg) if !driver_config_equal(old_cfg, new_cfg) => {
                    info!("driver '{}' config changed, restarting", name);
                    self.remove(name);
                }
                _ => {} // unchanged, keep running
            }
        }

        // Add new drivers (or restarted ones from above)
        let after_remove: HashMap<String, DriverConfig> = {
            let inner = self.inner.lock().unwrap();
            inner.iter().map(|(k, v)| (k.clone(), v.config.clone())).collect()
        };
        for cfg in new_configs {
            if !after_remove.contains_key(&cfg.name) {
                self.add(cfg.clone());
            }
        }
    }

    /// Send a command to a specific driver.
    pub fn send(&self, driver: &str, cmd: DriverCommand) -> Result<(), String> {
        let inner = self.inner.lock().unwrap();
        match inner.get(driver) {
            Some(d) => d.cmd_tx.send(cmd).map_err(|e| e.to_string()),
            None => Err(format!("driver '{}' not found", driver)),
        }
    }

    /// List current driver names.
    pub fn names(&self) -> Vec<String> {
        self.inner.lock().unwrap().keys().cloned().collect()
    }

    /// Get a snapshot of current driver configs.
    pub fn configs(&self) -> Vec<DriverConfig> {
        self.inner.lock().unwrap().values().map(|d| d.config.clone()).collect()
    }

    /// Shutdown all drivers (for graceful process exit).
    pub fn shutdown_all(&self) {
        let names: Vec<String> = self.inner.lock().unwrap().keys().cloned().collect();
        for name in names {
            self.remove(&name);
        }
    }
}

/// Pure diff: given old and new driver lists, return what to add, remove, restart.
/// Extracted so it can be unit-tested without spawning real driver threads.
pub fn diff_drivers(
    old: &[DriverConfig],
    new: &[DriverConfig],
) -> (Vec<DriverConfig>, Vec<String>, Vec<DriverConfig>) {
    let old_map: HashMap<&str, &DriverConfig> = old.iter().map(|c| (c.name.as_str(), c)).collect();
    let new_map: HashMap<&str, &DriverConfig> = new.iter().map(|c| (c.name.as_str(), c)).collect();

    let mut to_add = Vec::new();
    let mut to_remove = Vec::new();
    let mut to_restart = Vec::new();

    for (name, old_cfg) in &old_map {
        match new_map.get(name) {
            None => to_remove.push((*name).to_string()),
            Some(new_cfg) if !driver_config_equal(old_cfg, new_cfg) => {
                to_restart.push((*new_cfg).clone());
            }
            _ => {}
        }
    }
    for (name, new_cfg) in &new_map {
        if !old_map.contains_key(name) {
            to_add.push((*new_cfg).clone());
        }
    }
    (to_add, to_remove, to_restart)
}

fn driver_config_equal(a: &DriverConfig, b: &DriverConfig) -> bool {
    // Compare fields that matter for driver runtime
    a.lua == b.lua
        && a.is_site_meter == b.is_site_meter
        && a.battery_capacity_wh == b.battery_capacity_wh
        && mqtt_eq(&a.mqtt, &b.mqtt)
        && modbus_eq(&a.modbus, &b.modbus)
}

fn mqtt_eq(a: &Option<crate::config::MqttConnectionConfig>, b: &Option<crate::config::MqttConnectionConfig>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => x.host == y.host && x.port == y.port && x.username == y.username && x.password == y.password,
        _ => false,
    }
}

fn modbus_eq(a: &Option<crate::config::ModbusConnectionConfig>, b: &Option<crate::config::ModbusConnectionConfig>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => x.host == y.host && x.port == y.port && x.unit_id == y.unit_id,
        _ => false,
    }
}

/// Driver thread body — load Lua, init, poll loop, handle commands.
fn run_driver_thread(
    config: DriverConfig,
    store: Arc<Mutex<TelemetryStore>>,
    watchdog_timeout_s: u64,
    lua_dir: PathBuf,
    cmd_rx: mpsc::Receiver<DriverCommand>,
    running: Arc<std::sync::atomic::AtomicBool>,
) {
    info!("driver '{}': starting", config.name);

    let mut driver = match lua::driver::Driver::load(&config, store.clone(), watchdog_timeout_s, &lua_dir) {
        Ok(d) => d,
        Err(e) => {
            error!("driver '{}': failed to load: {}", config.name, e);
            store.lock().unwrap().driver_health_mut(&config.name).record_error(&e);
            store.lock().unwrap().driver_health_mut(&config.name).set_offline();
            return;
        }
    };

    if let Err(e) = driver.init(&config) {
        error!("driver '{}': init failed: {}", config.name, e);
    }

    info!("driver '{}': entering poll loop", config.name);

    while running.load(std::sync::atomic::Ordering::SeqCst) {
        // Drain commands
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                DriverCommand::Battery { power_w } => {
                    let cmd_json = r#"{"id":"ems"}"#;
                    if let Err(e) = driver.command("battery", power_w, cmd_json) {
                        warn!("driver '{}': command error: {}", config.name, e);
                    } else {
                        info!("driver '{}': battery -> {:.0}W", config.name, power_w);
                    }
                }
                DriverCommand::DefaultMode => {
                    if let Err(e) = driver.default_mode() {
                        warn!("driver '{}': default_mode error: {}", config.name, e);
                    }
                    driver.mark_watchdog_triggered();
                }
                DriverCommand::Shutdown => {
                    info!("driver '{}': shutdown received", config.name);
                    driver.cleanup();
                    return;
                }
            }
        }

        let interval = driver.poll();
        std::thread::sleep(interval);
    }

    driver.default_mode().ok();
    driver.cleanup();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DriverConfig, MqttConnectionConfig, ModbusConnectionConfig};

    fn mqtt_driver(name: &str, host: &str) -> DriverConfig {
        DriverConfig {
            name: name.into(),
            lua: format!("drivers/{}.lua", name),
            is_site_meter: false,
            battery_capacity_wh: 10000.0,
            mqtt: Some(MqttConnectionConfig {
                host: host.into(),
                port: 1883,
                username: None,
                password: None,
            }),
            modbus: None,
        }
    }

    fn modbus_driver(name: &str, host: &str) -> DriverConfig {
        DriverConfig {
            name: name.into(),
            lua: format!("drivers/{}.lua", name),
            is_site_meter: false,
            battery_capacity_wh: 9600.0,
            mqtt: None,
            modbus: Some(ModbusConnectionConfig {
                host: host.into(),
                port: 502,
                unit_id: 1,
            }),
        }
    }

    #[test]
    fn diff_no_change() {
        let cfgs = vec![mqtt_driver("a", "1.1.1.1"), modbus_driver("b", "2.2.2.2")];
        let (add, remove, restart) = diff_drivers(&cfgs, &cfgs);
        assert!(add.is_empty());
        assert!(remove.is_empty());
        assert!(restart.is_empty());
    }

    #[test]
    fn diff_pure_addition() {
        let old = vec![mqtt_driver("a", "1.1.1.1")];
        let new = vec![mqtt_driver("a", "1.1.1.1"), modbus_driver("b", "2.2.2.2")];
        let (add, remove, restart) = diff_drivers(&old, &new);
        assert_eq!(add.len(), 1);
        assert_eq!(add[0].name, "b");
        assert!(remove.is_empty());
        assert!(restart.is_empty());
    }

    #[test]
    fn diff_pure_removal() {
        let old = vec![mqtt_driver("a", "1.1.1.1"), modbus_driver("b", "2.2.2.2")];
        let new = vec![mqtt_driver("a", "1.1.1.1")];
        let (add, remove, restart) = diff_drivers(&old, &new);
        assert!(add.is_empty());
        assert_eq!(remove, vec!["b".to_string()]);
        assert!(restart.is_empty());
    }

    #[test]
    fn diff_mqtt_host_change_triggers_restart() {
        let old = vec![mqtt_driver("a", "1.1.1.1")];
        let new = vec![mqtt_driver("a", "9.9.9.9")];
        let (add, remove, restart) = diff_drivers(&old, &new);
        assert!(add.is_empty());
        assert!(remove.is_empty());
        assert_eq!(restart.len(), 1);
        assert_eq!(restart[0].mqtt.as_ref().unwrap().host, "9.9.9.9");
    }

    #[test]
    fn diff_modbus_unit_id_change_triggers_restart() {
        let mut old = modbus_driver("b", "2.2.2.2");
        let mut new = modbus_driver("b", "2.2.2.2");
        new.modbus.as_mut().unwrap().unit_id = 5;
        old.modbus.as_mut().unwrap().unit_id = 1;
        let (_, _, restart) = diff_drivers(&[old], &[new]);
        assert_eq!(restart.len(), 1);
        assert_eq!(restart[0].modbus.as_ref().unwrap().unit_id, 5);
    }

    #[test]
    fn diff_battery_capacity_change_triggers_restart() {
        let mut old = mqtt_driver("a", "1.1.1.1");
        let mut new = mqtt_driver("a", "1.1.1.1");
        old.battery_capacity_wh = 10000.0;
        new.battery_capacity_wh = 20000.0;
        let (_, _, restart) = diff_drivers(&[old], &[new]);
        assert_eq!(restart.len(), 1);
    }

    #[test]
    fn diff_lua_path_change_triggers_restart() {
        let mut old = mqtt_driver("a", "1.1.1.1");
        let mut new = mqtt_driver("a", "1.1.1.1");
        old.lua = "drivers/v1.lua".into();
        new.lua = "drivers/v2.lua".into();
        let (_, _, restart) = diff_drivers(&[old], &[new]);
        assert_eq!(restart.len(), 1);
    }

    #[test]
    fn diff_is_site_meter_change_triggers_restart() {
        let mut old = mqtt_driver("a", "1.1.1.1");
        let mut new = mqtt_driver("a", "1.1.1.1");
        old.is_site_meter = false;
        new.is_site_meter = true;
        let (_, _, restart) = diff_drivers(&[old], &[new]);
        assert_eq!(restart.len(), 1);
    }

    #[test]
    fn diff_protocol_swap_triggers_restart() {
        // Same name, but mqtt → modbus should be a restart
        let old = vec![mqtt_driver("dev", "1.1.1.1")];
        let new = vec![modbus_driver("dev", "1.1.1.1")];
        let (add, remove, restart) = diff_drivers(&old, &new);
        assert!(add.is_empty());
        assert!(remove.is_empty());
        assert_eq!(restart.len(), 1);
        assert!(restart[0].modbus.is_some());
    }

    #[test]
    fn diff_complex_mixed_changes() {
        // Old: a, b, c
        // New: a (changed), b (same), d (new) — c removed
        let mut a_old = mqtt_driver("a", "1.1.1.1");
        a_old.battery_capacity_wh = 10000.0;
        let mut a_new = mqtt_driver("a", "1.1.1.1");
        a_new.battery_capacity_wh = 15000.0;

        let b = modbus_driver("b", "2.2.2.2");
        let c = mqtt_driver("c", "3.3.3.3");
        let d = modbus_driver("d", "4.4.4.4");

        let old = vec![a_old, b.clone(), c];
        let new = vec![a_new, b, d];

        let (add, remove, restart) = diff_drivers(&old, &new);
        assert_eq!(add.len(), 1);
        assert_eq!(add[0].name, "d");
        assert_eq!(remove, vec!["c".to_string()]);
        assert_eq!(restart.len(), 1);
        assert_eq!(restart[0].name, "a");
    }

    #[test]
    fn driver_config_equal_ignores_irrelevant_fields() {
        // Username change SHOULD trigger restart (auth differs)
        let mut a = mqtt_driver("x", "1.1.1.1");
        let mut b = mqtt_driver("x", "1.1.1.1");
        a.mqtt.as_mut().unwrap().username = Some("u1".into());
        b.mqtt.as_mut().unwrap().username = Some("u2".into());
        assert!(!driver_config_equal(&a, &b));

        // No-op change is equal
        let a = mqtt_driver("x", "1.1.1.1");
        let b = mqtt_driver("x", "1.1.1.1");
        assert!(driver_config_equal(&a, &b));
    }
}
