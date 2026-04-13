use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub site: SiteConfig,
    pub fuse: FuseConfig,
    pub drivers: Vec<DriverConfig>,
    pub api: ApiConfig,
    #[serde(default)]
    pub homeassistant: Option<HomeAssistantConfig>,
    #[serde(default)]
    pub state: Option<StateConfig>,
    #[serde(default)]
    pub price: Option<PriceConfig>,
    #[serde(default)]
    pub weather: Option<WeatherConfig>,
    #[serde(default)]
    pub batteries: std::collections::HashMap<String, BatterySettings>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SiteConfig {
    pub name: String,
    #[serde(default = "default_control_interval")]
    pub control_interval_s: u64,
    #[serde(default)]
    pub grid_target_w: f64,
    #[serde(default = "default_tolerance")]
    pub grid_tolerance_w: f64,
    #[serde(default = "default_watchdog")]
    pub watchdog_timeout_s: u64,
    #[serde(default = "default_alpha")]
    pub smoothing_alpha: f64,
    #[serde(default = "default_gain")]
    pub gain: f64,
    #[serde(default = "default_slew_rate")]
    pub slew_rate_w: f64,
    #[serde(default = "default_dispatch_interval")]
    pub min_dispatch_interval_s: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FuseConfig {
    pub max_amps: f64,
    #[serde(default = "default_phases")]
    pub phases: u8,
    #[serde(default = "default_voltage")]
    pub voltage: f64,
}

impl FuseConfig {
    pub fn max_power_w(&self) -> f64 {
        self.max_amps * self.voltage * self.phases as f64
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DriverConfig {
    pub name: String,
    pub lua: String,
    #[serde(default)]
    pub is_site_meter: bool,
    #[serde(default)]
    pub battery_capacity_wh: f64,
    #[serde(default)]
    pub mqtt: Option<MqttConnectionConfig>,
    #[serde(default)]
    pub modbus: Option<ModbusConnectionConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MqttConnectionConfig {
    pub host: String,
    #[serde(default = "default_mqtt_port")]
    pub port: u16,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModbusConnectionConfig {
    pub host: String,
    #[serde(default = "default_modbus_port")]
    pub port: u16,
    #[serde(default = "default_unit_id")]
    pub unit_id: u8,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiConfig {
    #[serde(default = "default_api_port")]
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HomeAssistantConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub broker: String,
    #[serde(default = "default_mqtt_port")]
    pub port: u16,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default = "default_control_interval")]
    pub publish_interval_s: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StateConfig {
    #[serde(default = "default_state_path")]
    pub path: String,
}

/// Spot price configuration.
/// provider: "elprisetjustnu" (no key, free, only Sweden), "entsoe" (needs key), "none"
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PriceConfig {
    #[serde(default = "default_price_provider")]
    pub provider: String,
    #[serde(default = "default_price_zone")]
    pub zone: String,                 // SE1, SE2, SE3, SE4
    #[serde(default)]
    pub grid_tariff_ore_kwh: f64,     // fixed grid tariff (öre/kWh)
    #[serde(default = "default_vat")]
    pub vat_percent: f64,             // 25% in Sweden
    #[serde(default)]
    pub api_key: Option<String>,      // for ENTSO-E
}

/// Weather forecast configuration.
/// provider: "met_no" (no key, free), "openweather", "none"
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WeatherConfig {
    #[serde(default = "default_weather_provider")]
    pub provider: String,
    pub latitude: f64,
    pub longitude: f64,
    #[serde(default)]
    pub api_key: Option<String>,
}

/// Per-battery settings (overrides hardcoded defaults).
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct BatterySettings {
    pub soc_min: Option<f64>,         // 0.0-1.0, default: no floor (BMS handles)
    pub soc_max: Option<f64>,         // 0.0-1.0, default: no ceiling (BMS handles)
    pub max_charge_w: Option<f64>,
    pub max_discharge_w: Option<f64>,
    pub weight: Option<f64>,          // for weighted mode (default: 1.0)
}

// Defaults
fn default_control_interval() -> u64 { 5 }
fn default_tolerance() -> f64 { 42.0 } // The Answer
fn default_watchdog() -> u64 { 60 }
fn default_alpha() -> f64 { 0.3 }
fn default_gain() -> f64 { 0.5 }
fn default_slew_rate() -> f64 { 500.0 }
fn default_dispatch_interval() -> u64 { 5 }
fn default_phases() -> u8 { 3 }
fn default_voltage() -> f64 { 230.0 }
fn default_mqtt_port() -> u16 { 1883 }
fn default_modbus_port() -> u16 { 502 }
fn default_unit_id() -> u8 { 1 }
fn default_api_port() -> u16 { 8080 }
fn default_true() -> bool { true }
fn default_state_path() -> String { "state.redb".to_string() }
fn default_price_provider() -> String { "elprisetjustnu".to_string() }
fn default_price_zone() -> String { "SE3".to_string() }
fn default_vat() -> f64 { 25.0 }
fn default_weather_provider() -> String { "met_no".to_string() }

impl Config {
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = std::fs::read_to_string(path)?;
        let config: Config = serde_yaml::from_str(&contents)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), Box<dyn std::error::Error>> {
        if self.drivers.is_empty() {
            return Err("at least one driver must be configured".into());
        }

        let site_meters: Vec<_> = self.drivers.iter().filter(|d| d.is_site_meter).collect();
        if site_meters.is_empty() {
            return Err("at least one driver must be marked as is_site_meter".into());
        }

        for driver in &self.drivers {
            if driver.mqtt.is_none() && driver.modbus.is_none() {
                return Err(format!("driver '{}' must have either mqtt or modbus config", driver.name).into());
            }
        }

        if self.site.smoothing_alpha <= 0.0 || self.site.smoothing_alpha > 1.0 {
            return Err("smoothing_alpha must be between 0 (exclusive) and 1 (inclusive)".into());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_yaml() -> &'static str {
        r#"
site:
  name: "Test"
fuse:
  max_amps: 16
drivers:
  - name: a
    lua: drivers/a.lua
    is_site_meter: true
    mqtt:
      host: 192.168.1.10
api:
  port: 8080
"#
    }

    #[test]
    fn loads_minimal_yaml_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.yaml");
        std::fs::write(&path, minimal_yaml()).unwrap();
        let cfg = Config::load(&path).expect("must load");

        assert_eq!(cfg.site.name, "Test");
        assert_eq!(cfg.site.control_interval_s, 5);
        assert_eq!(cfg.site.grid_target_w, 0.0);
        assert_eq!(cfg.site.grid_tolerance_w, 42.0); // The Answer
        assert_eq!(cfg.site.watchdog_timeout_s, 60);
        assert_eq!(cfg.site.smoothing_alpha, 0.3);
        assert_eq!(cfg.site.slew_rate_w, 500.0);
        assert_eq!(cfg.site.min_dispatch_interval_s, 5);

        assert_eq!(cfg.fuse.max_amps, 16.0);
        assert_eq!(cfg.fuse.phases, 3);
        assert_eq!(cfg.fuse.voltage, 230.0);
        assert_eq!(cfg.fuse.max_power_w(), 16.0 * 230.0 * 3.0);

        assert_eq!(cfg.drivers.len(), 1);
        assert!(cfg.drivers[0].is_site_meter);
        assert_eq!(cfg.drivers[0].mqtt.as_ref().unwrap().port, 1883); // default

        // Optional sections default to None / empty
        assert!(cfg.homeassistant.is_none());
        assert!(cfg.price.is_none());
        assert!(cfg.weather.is_none());
        assert!(cfg.batteries.is_empty());
    }

    #[test]
    fn validate_rejects_no_drivers() {
        let yaml = r#"
site: { name: "x" }
fuse: { max_amps: 16 }
drivers: []
api: { port: 8080 }
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.yaml");
        std::fs::write(&path, yaml).unwrap();
        let err = Config::load(&path).unwrap_err();
        assert!(err.to_string().contains("at least one driver"));
    }

    #[test]
    fn validate_rejects_no_site_meter() {
        let yaml = r#"
site: { name: "x" }
fuse: { max_amps: 16 }
drivers:
  - name: a
    lua: a.lua
    mqtt: { host: 1.1.1.1 }
api: { port: 8080 }
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.yaml");
        std::fs::write(&path, yaml).unwrap();
        let err = Config::load(&path).unwrap_err();
        assert!(err.to_string().contains("is_site_meter"));
    }

    #[test]
    fn validate_rejects_driver_without_protocol() {
        let yaml = r#"
site: { name: "x" }
fuse: { max_amps: 16 }
drivers:
  - name: a
    lua: a.lua
    is_site_meter: true
api: { port: 8080 }
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.yaml");
        std::fs::write(&path, yaml).unwrap();
        let err = Config::load(&path).unwrap_err();
        assert!(err.to_string().contains("mqtt or modbus"));
    }

    #[test]
    fn validate_rejects_bad_smoothing_alpha() {
        for bad in &[0.0, -0.1, 1.5] {
            let yaml = format!(
                r#"
site: {{ name: "x", smoothing_alpha: {} }}
fuse: {{ max_amps: 16 }}
drivers:
  - name: a
    lua: a.lua
    is_site_meter: true
    mqtt: {{ host: 1.1.1.1 }}
api: {{ port: 8080 }}
"#,
                bad
            );
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("c.yaml");
            std::fs::write(&path, yaml).unwrap();
            let err = Config::load(&path).unwrap_err();
            assert!(err.to_string().contains("smoothing_alpha"), "expected smoothing_alpha err for {}", bad);
        }
    }

    #[test]
    fn yaml_roundtrip_preserves_all_sections() {
        let original = Config {
            site: SiteConfig {
                name: "RoundTrip".into(),
                control_interval_s: 10,
                grid_target_w: -200.0,
                grid_tolerance_w: 75.0,
                watchdog_timeout_s: 90,
                smoothing_alpha: 0.5,
                gain: 0.6,
                slew_rate_w: 300.0,
                min_dispatch_interval_s: 7,
            },
            fuse: FuseConfig { max_amps: 20.0, phases: 1, voltage: 230.0 },
            drivers: vec![DriverConfig {
                name: "ferroamp".into(),
                lua: "drivers/ferroamp.lua".into(),
                is_site_meter: true,
                battery_capacity_wh: 15200.0,
                mqtt: Some(MqttConnectionConfig {
                    host: "10.0.0.1".into(),
                    port: 1884,
                    username: Some("u".into()),
                    password: Some("p".into()),
                }),
                modbus: None,
            }],
            api: ApiConfig { port: 9090 },
            homeassistant: Some(HomeAssistantConfig {
                enabled: true,
                broker: "10.0.0.2".into(),
                port: 1883,
                username: None,
                password: None,
                publish_interval_s: 5,
            }),
            state: Some(StateConfig { path: "/tmp/state.redb".into() }),
            price: Some(PriceConfig {
                provider: "elprisetjustnu".into(),
                zone: "SE3".into(),
                grid_tariff_ore_kwh: 50.0,
                vat_percent: 25.0,
                api_key: None,
            }),
            weather: Some(WeatherConfig {
                provider: "met_no".into(),
                latitude: 59.3293,
                longitude: 18.0686,
                api_key: None,
            }),
            batteries: {
                let mut m = std::collections::HashMap::new();
                m.insert("ferroamp".into(), BatterySettings {
                    soc_min: Some(0.1),
                    soc_max: Some(0.95),
                    max_charge_w: Some(5000.0),
                    max_discharge_w: Some(5000.0),
                    weight: Some(2.0),
                });
                m
            },
        };

        let yaml = serde_yaml::to_string(&original).unwrap();
        let parsed: Config = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(parsed.site.name, "RoundTrip");
        assert_eq!(parsed.site.grid_target_w, -200.0);
        assert_eq!(parsed.fuse.max_amps, 20.0);
        assert_eq!(parsed.drivers.len(), 1);
        assert_eq!(parsed.drivers[0].mqtt.as_ref().unwrap().host, "10.0.0.1");
        assert_eq!(parsed.api.port, 9090);
        assert_eq!(parsed.homeassistant.as_ref().unwrap().broker, "10.0.0.2");
        assert_eq!(parsed.state.as_ref().unwrap().path, "/tmp/state.redb");
        assert_eq!(parsed.price.as_ref().unwrap().zone, "SE3");
        assert_eq!(parsed.weather.as_ref().unwrap().latitude, 59.3293);
        assert_eq!(parsed.batteries["ferroamp"].weight, Some(2.0));
    }

    #[test]
    fn fuse_max_power_calculation() {
        let f = FuseConfig { max_amps: 16.0, phases: 3, voltage: 230.0 };
        assert_eq!(f.max_power_w(), 11040.0);
        let f1 = FuseConfig { max_amps: 25.0, phases: 1, voltage: 230.0 };
        assert_eq!(f1.max_power_w(), 5750.0);
    }

    #[test]
    fn forward_compatible_unknown_fields_in_extension_sections() {
        // Adding a field to PriceConfig in the future shouldn't break parsing
        // of existing yaml; we use serde(default) on the section itself.
        let yaml = r#"
site: { name: "x" }
fuse: { max_amps: 16 }
drivers:
  - name: a
    lua: a.lua
    is_site_meter: true
    mqtt: { host: 1.1.1.1 }
api: { port: 8080 }
price:
  provider: entsoe
  zone: SE2
  api_key: secret123
"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.yaml");
        std::fs::write(&path, yaml).unwrap();
        let cfg = Config::load(&path).unwrap();
        let p = cfg.price.expect("price section present");
        assert_eq!(p.provider, "entsoe");
        assert_eq!(p.zone, "SE2");
        assert_eq!(p.vat_percent, 25.0); // default
        assert_eq!(p.api_key.as_deref(), Some("secret123"));
    }
}
