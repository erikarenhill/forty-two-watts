use std::collections::HashMap;
use std::time::Instant;

/// DER type classification
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DerType {
    Meter,
    Pv,
    Battery,
}

impl DerType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "meter" => Some(Self::Meter),
            "pv" => Some(Self::Pv),
            "battery" => Some(Self::Battery),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Meter => "meter",
            Self::Pv => "pv",
            Self::Battery => "battery",
        }
    }
}

/// Driver health status
#[derive(Debug, Clone, PartialEq)]
pub enum DriverStatus {
    Ok,
    Degraded,
    Offline,
}

/// A single DER reading with smoothing
#[derive(Debug, Clone)]
pub struct DerReading {
    pub driver: String,
    pub der_type: DerType,
    pub raw_w: f64,
    pub smoothed_w: f64,
    pub soc: Option<f64>,
    pub data: serde_json::Value,
    pub updated_at: Instant,
}

/// Per-driver health tracking
#[derive(Debug, Clone)]
pub struct DriverHealth {
    pub name: String,
    pub status: DriverStatus,
    pub last_success: Option<Instant>,
    pub consecutive_errors: u32,
    pub last_error: Option<String>,
    pub tick_count: u64,
}

impl DriverHealth {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            status: DriverStatus::Ok,
            last_success: None,
            consecutive_errors: 0,
            last_error: None,
            tick_count: 0,
        }
    }

    pub fn record_success(&mut self) {
        self.last_success = Some(Instant::now());
        self.consecutive_errors = 0;
        self.last_error = None;
        self.status = DriverStatus::Ok;
        self.tick_count += 1;
    }

    pub fn record_error(&mut self, err: &str) {
        self.consecutive_errors += 1;
        self.last_error = Some(err.to_string());
        self.tick_count += 1;

        if self.consecutive_errors >= 3 {
            self.status = DriverStatus::Degraded;
        }
    }

    pub fn set_offline(&mut self) {
        self.status = DriverStatus::Offline;
    }

    pub fn is_online(&self) -> bool {
        self.status != DriverStatus::Offline
    }
}

/// Central telemetry store — shared across threads via Arc<Mutex<TelemetryStore>>
pub struct TelemetryStore {
    /// Key: "driver_name:der_type" (e.g. "ferroamp:meter")
    readings: HashMap<String, DerReading>,
    /// Per-driver health
    health: HashMap<String, DriverHealth>,
    /// EMA smoothing alpha
    alpha: f64,
}

impl TelemetryStore {
    pub fn new(alpha: f64) -> Self {
        Self {
            readings: HashMap::new(),
            health: HashMap::new(),
            alpha,
        }
    }

    fn key(driver: &str, der_type: &DerType) -> String {
        format!("{}:{}", driver, der_type.as_str())
    }

    /// Update a DER reading with EMA smoothing
    pub fn update(&mut self, driver: &str, der_type: &DerType, data: serde_json::Value, raw_w: f64, soc: Option<f64>) {
        let key = Self::key(driver, der_type);

        let smoothed_w = if let Some(prev) = self.readings.get(&key) {
            self.alpha * raw_w + (1.0 - self.alpha) * prev.smoothed_w
        } else {
            raw_w
        };

        self.readings.insert(key, DerReading {
            driver: driver.to_string(),
            der_type: der_type.clone(),
            raw_w,
            smoothed_w,
            soc,
            data,
            updated_at: Instant::now(),
        });
    }

    /// Get a specific reading
    pub fn get(&self, driver: &str, der_type: &DerType) -> Option<&DerReading> {
        self.readings.get(&Self::key(driver, der_type))
    }

    /// Get all readings of a specific DER type
    pub fn readings_by_type(&self, der_type: &DerType) -> Vec<&DerReading> {
        self.readings.values()
            .filter(|r| &r.der_type == der_type)
            .collect()
    }

    /// Get all readings for a driver
    pub fn readings_by_driver(&self, driver: &str) -> Vec<&DerReading> {
        self.readings.values()
            .filter(|r| r.driver == driver)
            .collect()
    }

    /// Get or create driver health tracker
    pub fn driver_health_mut(&mut self, name: &str) -> &mut DriverHealth {
        self.health.entry(name.to_string())
            .or_insert_with(|| DriverHealth::new(name))
    }

    /// Get driver health
    pub fn driver_health(&self, name: &str) -> Option<&DriverHealth> {
        self.health.get(name)
    }

    /// Get all driver health entries
    pub fn all_health(&self) -> &HashMap<String, DriverHealth> {
        &self.health
    }

    /// Check if a reading is stale (older than timeout)
    pub fn is_stale(&self, driver: &str, der_type: &DerType, timeout_s: u64) -> bool {
        match self.get(driver, der_type) {
            Some(reading) => reading.updated_at.elapsed().as_secs() > timeout_s,
            None => true,
        }
    }
}
