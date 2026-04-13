use redb::{Database, ReadableDatabase, TableDefinition, ReadableTable};
use tracing::{info, warn, error};

/// Average numeric fields in a bucket of JSON snapshots. Returns the average
/// as a new JSON string with the middle timestamp.
fn average_json_bucket(chunk: &[(u64, String)]) -> Option<(u64, String)> {
    if chunk.is_empty() { return None; }
    if chunk.len() == 1 { return Some(chunk[0].clone()); }

    // Parse all as JSON objects
    let mut sums: std::collections::HashMap<String, (f64, usize)> = std::collections::HashMap::new();
    let mut first_obj: Option<serde_json::Map<String, serde_json::Value>> = None;

    for (_, json) in chunk {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(json) {
            if let Some(obj) = val.as_object() {
                if first_obj.is_none() {
                    first_obj = Some(obj.clone());
                }
                collect_numbers("", obj, &mut sums);
            }
        }
    }

    // Build averaged object by replacing numeric fields in the first object
    let mut result = first_obj?;
    replace_with_averages("", &mut result, &sums);

    let mid_ts = chunk[chunk.len() / 2].0;
    Some((mid_ts, serde_json::Value::Object(result).to_string()))
}

fn collect_numbers(prefix: &str, obj: &serde_json::Map<String, serde_json::Value>, sums: &mut std::collections::HashMap<String, (f64, usize)>) {
    for (k, v) in obj {
        let path = if prefix.is_empty() { k.clone() } else { format!("{}.{}", prefix, k) };
        match v {
            serde_json::Value::Number(n) => {
                if let Some(f) = n.as_f64() {
                    let entry = sums.entry(path).or_insert((0.0, 0));
                    entry.0 += f;
                    entry.1 += 1;
                }
            }
            serde_json::Value::Object(inner) => {
                collect_numbers(&path, inner, sums);
            }
            _ => {}
        }
    }
}

fn replace_with_averages(prefix: &str, obj: &mut serde_json::Map<String, serde_json::Value>, sums: &std::collections::HashMap<String, (f64, usize)>) {
    for (k, v) in obj.iter_mut() {
        let path = if prefix.is_empty() { k.clone() } else { format!("{}.{}", prefix, k) };
        match v {
            serde_json::Value::Number(_) => {
                if let Some((sum, count)) = sums.get(&path) {
                    if *count > 0 {
                        if let Some(n) = serde_json::Number::from_f64(sum / *count as f64) {
                            *v = serde_json::Value::Number(n);
                        }
                    }
                }
            }
            serde_json::Value::Object(inner) => {
                replace_with_averages(&path, inner, sums);
            }
            _ => {}
        }
    }
}

const CONFIG_TABLE: TableDefinition<&str, &str> = TableDefinition::new("config");
const TELEMETRY_TABLE: TableDefinition<&str, &str> = TableDefinition::new("telemetry");
const EVENTS_TABLE: TableDefinition<u64, &str> = TableDefinition::new("events");
const HISTORY_TABLE: TableDefinition<u64, &str> = TableDefinition::new("history");

/// How long to keep history (3 days in seconds)
pub const HISTORY_RETENTION_S: u64 = 3 * 24 * 3600;

/// Persistent state store backed by redb
pub struct StateStore {
    db: Database,
}

impl StateStore {
    pub fn open(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let db = Database::create(path)?;

        // Ensure tables exist
        let txn = db.begin_write()?;
        {
            let _ = txn.open_table(CONFIG_TABLE)?;
            let _ = txn.open_table(TELEMETRY_TABLE)?;
            let _ = txn.open_table(EVENTS_TABLE)?;
            let _ = txn.open_table(HISTORY_TABLE)?;
        }
        txn.commit()?;

        info!("state store opened: {}", path);
        Ok(Self { db })
    }

    /// Save a config value (mode, grid target, weights, etc.)
    pub fn save_config(&self, key: &str, value: &str) {
        match self.db.begin_write() {
            Ok(txn) => {
                match txn.open_table(CONFIG_TABLE) {
                    Ok(mut table) => {
                        if let Err(e) = table.insert(key, value) {
                            error!("failed to save config {}: {}", key, e);
                        }
                    }
                    Err(e) => error!("failed to open config table: {}", e),
                }
                if let Err(e) = txn.commit() {
                    error!("failed to commit config: {}", e);
                }
            }
            Err(e) => error!("failed to begin write txn: {}", e),
        }
    }

    /// Load a config value
    pub fn load_config(&self, key: &str) -> Option<String> {
        match self.db.begin_read() {
            Ok(txn) => {
                match txn.open_table(CONFIG_TABLE) {
                    Ok(table) => {
                        table.get(key).ok().flatten().map(|v| v.value().to_string())
                    }
                    Err(_) => None,
                }
            }
            Err(_) => None,
        }
    }

    /// Save last known telemetry for crash recovery
    pub fn save_telemetry(&self, key: &str, json: &str) {
        match self.db.begin_write() {
            Ok(txn) => {
                match txn.open_table(TELEMETRY_TABLE) {
                    Ok(mut table) => {
                        if let Err(e) = table.insert(key, json) {
                            warn!("failed to save telemetry {}: {}", key, e);
                        }
                    }
                    Err(e) => warn!("failed to open telemetry table: {}", e),
                }
                let _ = txn.commit();
            }
            Err(e) => warn!("failed to begin write txn: {}", e),
        }
    }

    /// Load last known telemetry
    pub fn load_telemetry(&self, key: &str) -> Option<String> {
        match self.db.begin_read() {
            Ok(txn) => {
                match txn.open_table(TELEMETRY_TABLE) {
                    Ok(table) => {
                        table.get(key).ok().flatten().map(|v| v.value().to_string())
                    }
                    Err(_) => None,
                }
            }
            Err(_) => None,
        }
    }

    /// Record an event (mode change, error, recovery)
    pub fn record_event(&self, event: &str) {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        match self.db.begin_write() {
            Ok(txn) => {
                match txn.open_table(EVENTS_TABLE) {
                    Ok(mut table) => {
                        if let Err(e) = table.insert(timestamp, event) {
                            warn!("failed to record event: {}", e);
                        }
                    }
                    Err(e) => warn!("failed to open events table: {}", e),
                }
                let _ = txn.commit();
            }
            Err(e) => warn!("failed to begin write txn: {}", e),
        }
    }

    /// Record a telemetry snapshot to history.
    /// ts_ms: unix timestamp in milliseconds
    /// json: JSON-encoded snapshot (grid_w, pv_w, bat_w, load_w, bat_soc, drivers...)
    pub fn record_history(&self, ts_ms: u64, json: &str) {
        match self.db.begin_write() {
            Ok(txn) => {
                match txn.open_table(HISTORY_TABLE) {
                    Ok(mut table) => {
                        if let Err(e) = table.insert(ts_ms, json) {
                            warn!("history insert: {}", e);
                        }
                    }
                    Err(e) => warn!("history table: {}", e),
                }
                let _ = txn.commit();
            }
            Err(e) => warn!("history write txn: {}", e),
        }
    }

    /// Load history entries in range [since_ms, until_ms], optionally downsampled
    /// to at most `max_points` entries by bucket averaging.
    pub fn load_history(&self, since_ms: u64, until_ms: u64, max_points: usize) -> Vec<(u64, String)> {
        let all: Vec<(u64, String)> = match self.db.begin_read() {
            Ok(txn) => match txn.open_table(HISTORY_TABLE) {
                Ok(table) => match table.range(since_ms..=until_ms) {
                    Ok(iter) => iter
                        .filter_map(|r| r.ok())
                        .map(|(k, v)| (k.value(), v.value().to_string()))
                        .collect(),
                    Err(_) => Vec::new(),
                },
                Err(_) => Vec::new(),
            },
            Err(_) => Vec::new(),
        };

        if all.len() <= max_points || max_points == 0 {
            return all;
        }

        // Downsample by bucket averaging of numeric fields in the JSON
        let bucket_size = all.len().div_ceil(max_points);
        let mut result = Vec::with_capacity(max_points);
        for chunk in all.chunks(bucket_size) {
            if let Some(avg) = average_json_bucket(chunk) {
                result.push(avg);
            }
        }
        result
    }

    /// Delete history entries older than `retention_s` seconds
    pub fn prune_history(&self, retention_s: u64) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let cutoff = now_ms.saturating_sub(retention_s * 1000);

        match self.db.begin_write() {
            Ok(txn) => {
                match txn.open_table(HISTORY_TABLE) {
                    Ok(mut table) => {
                        let _ = table.retain(|k, _| k >= cutoff);
                    }
                    Err(e) => warn!("prune: {}", e),
                }
                let _ = txn.commit();
            }
            Err(e) => warn!("prune txn: {}", e),
        }
    }

    /// Count history entries (for diagnostics)
    pub fn history_count(&self) -> usize {
        match self.db.begin_read() {
            Ok(txn) => match txn.open_table(HISTORY_TABLE) {
                Ok(table) => table.iter().map(|it| it.count()).unwrap_or(0),
                Err(_) => 0,
            },
            Err(_) => 0,
        }
    }

    /// Load recent events (last N)
    pub fn recent_events(&self, limit: usize) -> Vec<(u64, String)> {
        let mut events = Vec::new();

        match self.db.begin_read() {
            Ok(txn) => {
                match txn.open_table(EVENTS_TABLE) {
                    Ok(table) => {
                        // Iterate in reverse (most recent first)
                        if let Ok(iter) = table.iter() {
                            let all: Vec<_> = iter
                                .filter_map(|r| r.ok())
                                .map(|(k, v)| (k.value(), v.value().to_string()))
                                .collect();
                            let start = if all.len() > limit { all.len() - limit } else { 0 };
                            events = all[start..].to_vec();
                        }
                    }
                    Err(_) => {}
                }
            }
            Err(_) => {}
        }

        events
    }
}
