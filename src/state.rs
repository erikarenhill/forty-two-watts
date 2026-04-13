use redb::{Database, ReadableDatabase, TableDefinition, ReadableTable};
use tracing::{info, warn, error};

/// Group entries into time buckets of `bucket_ms` and average each bucket.
/// Returns (bucket_start_ms, averaged_json) pairs.
fn bucket_by_time(entries: &[(u64, String)], bucket_ms: u64) -> Vec<(u64, String)> {
    let mut buckets: std::collections::BTreeMap<u64, Vec<(u64, String)>> = std::collections::BTreeMap::new();
    for (ts, json) in entries {
        let bucket = (*ts / bucket_ms) * bucket_ms;
        buckets.entry(bucket).or_default().push((*ts, json.clone()));
    }
    buckets.into_iter()
        .filter_map(|(bucket_ts, chunk)| {
            average_json_bucket(&chunk).map(|(_, json)| (bucket_ts, json))
        })
        .collect()
}

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

// Tiered history tables
const HISTORY_HOT: TableDefinition<u64, &str> = TableDefinition::new("history");        // 5s native, 30 days
const HISTORY_WARM: TableDefinition<u64, &str> = TableDefinition::new("history_warm");  // 15min buckets, 12 months
const HISTORY_COLD: TableDefinition<u64, &str> = TableDefinition::new("history_cold");  // daily buckets, forever

/// Hot retention: 30 days at 5s resolution
pub const HOT_RETENTION_S: u64 = 30 * 24 * 3600;
/// Warm retention: 12 months at 15min resolution
pub const WARM_RETENTION_S: u64 = 365 * 24 * 3600;
/// Warm bucket size in ms
const WARM_BUCKET_MS: u64 = 15 * 60 * 1000;
/// Cold bucket size in ms (1 day)
const COLD_BUCKET_MS: u64 = 24 * 3600 * 1000;

/// Legacy alias (used in existing main.rs)
pub const HISTORY_RETENTION_S: u64 = HOT_RETENTION_S;

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
            let _ = txn.open_table(HISTORY_HOT)?;
            let _ = txn.open_table(HISTORY_WARM)?;
            let _ = txn.open_table(HISTORY_COLD)?;
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

    /// Record a telemetry snapshot to the hot history tier.
    pub fn record_history(&self, ts_ms: u64, json: &str) {
        match self.db.begin_write() {
            Ok(txn) => {
                match txn.open_table(HISTORY_HOT) {
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

    /// Load history entries in range [since_ms, until_ms] from all tiers (hot+warm+cold),
    /// merged by timestamp, optionally downsampled to at most `max_points` entries.
    pub fn load_history(&self, since_ms: u64, until_ms: u64, max_points: usize) -> Vec<(u64, String)> {
        let mut all: Vec<(u64, String)> = Vec::new();
        if let Ok(txn) = self.db.begin_read() {
            for tbl in [HISTORY_COLD, HISTORY_WARM, HISTORY_HOT] {
                if let Ok(table) = txn.open_table(tbl) {
                    if let Ok(iter) = table.range(since_ms..=until_ms) {
                        for r in iter.flatten() {
                            all.push((r.0.value(), r.1.value().to_string()));
                        }
                    }
                }
            }
        }
        all.sort_by_key(|(k, _)| *k);
        // Deduplicate overlapping timestamps (prefer hot → warm → cold, last wins)
        all.dedup_by_key(|(k, _)| *k);

        if all.len() <= max_points || max_points == 0 {
            return all;
        }

        // Downsample by bucket averaging
        let bucket_size = all.len().div_ceil(max_points);
        let mut result = Vec::with_capacity(max_points);
        for chunk in all.chunks(bucket_size) {
            if let Some(avg) = average_json_bucket(chunk) {
                result.push(avg);
            }
        }
        result
    }

    /// Tiered retention: aggregate old hot data into warm tier, old warm into cold, prune both.
    /// Called periodically by the control loop.
    pub fn prune_history(&self, retention_s: u64) {
        let _ = retention_s; // legacy param, ignored — use constants
        if let Err(e) = self.do_tiered_maintenance() {
            warn!("tiered maintenance: {}", e);
        }
    }

    fn do_tiered_maintenance(&self) -> Result<(), Box<dyn std::error::Error>> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let hot_cutoff = now_ms.saturating_sub(HOT_RETENTION_S * 1000);
        let warm_cutoff = now_ms.saturating_sub(WARM_RETENTION_S * 1000);

        // 1) Read hot entries older than cutoff
        let hot_old: Vec<(u64, String)> = {
            let txn = self.db.begin_read()?;
            let table = txn.open_table(HISTORY_HOT)?;
            table.range(..hot_cutoff)?
                .filter_map(|r| r.ok())
                .map(|(k, v)| (k.value(), v.value().to_string()))
                .collect()
        };

        if !hot_old.is_empty() {
            // Aggregate into 15-min buckets, write to warm
            let warm_buckets = bucket_by_time(&hot_old, WARM_BUCKET_MS);
            let txn = self.db.begin_write()?;
            {
                let mut warm = txn.open_table(HISTORY_WARM)?;
                for (ts, json) in &warm_buckets {
                    warm.insert(*ts, json.as_str())?;
                }
                // Delete from hot
                let mut hot = txn.open_table(HISTORY_HOT)?;
                hot.retain(|k, _| k >= hot_cutoff)?;
            }
            txn.commit()?;
            info!("tiered: {} hot samples → {} warm buckets (15min)",
                hot_old.len(), warm_buckets.len());
        }

        // 2) Read warm entries older than cutoff
        let warm_old: Vec<(u64, String)> = {
            let txn = self.db.begin_read()?;
            let table = txn.open_table(HISTORY_WARM)?;
            table.range(..warm_cutoff)?
                .filter_map(|r| r.ok())
                .map(|(k, v)| (k.value(), v.value().to_string()))
                .collect()
        };

        if !warm_old.is_empty() {
            // Aggregate into daily buckets, write to cold
            let cold_buckets = bucket_by_time(&warm_old, COLD_BUCKET_MS);
            let txn = self.db.begin_write()?;
            {
                let mut cold = txn.open_table(HISTORY_COLD)?;
                for (ts, json) in &cold_buckets {
                    cold.insert(*ts, json.as_str())?;
                }
                // Delete from warm
                let mut warm = txn.open_table(HISTORY_WARM)?;
                warm.retain(|k, _| k >= warm_cutoff)?;
            }
            txn.commit()?;
            info!("tiered: {} warm samples → {} cold buckets (1d)",
                warm_old.len(), cold_buckets.len());
        }

        Ok(())
    }

    /// Count history entries per tier (for diagnostics)
    pub fn history_counts(&self) -> (usize, usize, usize) {
        let mut counts = (0, 0, 0);
        if let Ok(txn) = self.db.begin_read() {
            if let Ok(t) = txn.open_table(HISTORY_HOT) { counts.0 = t.iter().map(|it| it.count()).unwrap_or(0); }
            if let Ok(t) = txn.open_table(HISTORY_WARM) { counts.1 = t.iter().map(|it| it.count()).unwrap_or(0); }
            if let Ok(t) = txn.open_table(HISTORY_COLD) { counts.2 = t.iter().map(|it| it.count()).unwrap_or(0); }
        }
        counts
    }

    pub fn history_count(&self) -> usize {
        let (h, w, c) = self.history_counts();
        h + w + c
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
