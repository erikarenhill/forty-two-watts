//! Energy accumulator — integrates power (W) over time to get energy (kWh)
//!
//! Tracks: import, export, PV, battery charge, battery discharge
//! Two periods: today (resets at local midnight) and total (all-time)

use std::time::Instant;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct EnergyCounters {
    pub import_wh: f64,
    pub export_wh: f64,
    pub pv_wh: f64,
    pub bat_charged_wh: f64,
    pub bat_discharged_wh: f64,
    pub load_wh: f64,
}

impl EnergyCounters {
    /// Integrate power values over a time delta (in seconds)
    /// grid_w: positive=import, negative=export
    /// pv_w: negative=generation (convention)
    /// bat_w: positive=charge, negative=discharge
    pub fn integrate(&mut self, grid_w: f64, pv_w: f64, bat_w: f64, dt_s: f64) {
        // Convert W*s to Wh (divide by 3600)
        let factor = dt_s / 3600.0;

        if grid_w > 0.0 {
            self.import_wh += grid_w * factor;
        } else {
            self.export_wh += -grid_w * factor;
        }

        // PV: absolute value (generation magnitude)
        self.pv_wh += pv_w.abs() * factor;

        if bat_w > 0.0 {
            self.bat_charged_wh += bat_w * factor;
        } else {
            self.bat_discharged_wh += -bat_w * factor;
        }

        // Load = grid - pv - bat (house consumption, energy balance)
        // Clamp to positive since load can't be negative
        let load_w = (grid_w - pv_w - bat_w).max(0.0);
        self.load_wh += load_w * factor;
    }
}

/// Complete energy state with today + all-time counters
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct EnergyState {
    pub today: EnergyCounters,
    pub total: EnergyCounters,
    /// Date string (YYYY-MM-DD) for today's counters — used to detect day rollover
    pub today_date: String,
}

impl EnergyState {
    /// Check if we've crossed midnight; if so, reset today counters
    pub fn check_day_rollover(&mut self) {
        let current = current_date_string();
        if self.today_date != current {
            if !self.today_date.is_empty() {
                tracing::info!("day rollover: {} → {}, today={:.2} kWh import, {:.2} kWh PV",
                    self.today_date, current,
                    self.today.import_wh / 1000.0,
                    self.today.pv_wh / 1000.0);
            }
            self.today = EnergyCounters::default();
            self.today_date = current;
        }
    }

    /// Integrate a reading into both today and total
    pub fn integrate(&mut self, grid_w: f64, pv_w: f64, bat_w: f64, dt_s: f64) {
        self.check_day_rollover();
        self.today.integrate(grid_w, pv_w, bat_w, dt_s);
        self.total.integrate(grid_w, pv_w, bat_w, dt_s);
    }
}

/// Accumulator with timing — integrates using elapsed time since last call
pub struct EnergyAccumulator {
    pub state: EnergyState,
    last_integrate: Option<Instant>,
}

impl EnergyAccumulator {
    pub fn new(state: EnergyState) -> Self {
        Self {
            state,
            last_integrate: None,
        }
    }

    /// Integrate current power readings. Uses elapsed time since last call.
    /// Skips if the gap is too large (> 60s — probably a restart, don't inflate counters).
    pub fn integrate(&mut self, grid_w: f64, pv_w: f64, bat_w: f64) {
        let now = Instant::now();
        if let Some(last) = self.last_integrate {
            let dt = now.duration_since(last).as_secs_f64();
            if dt > 0.0 && dt < 60.0 {
                self.state.integrate(grid_w, pv_w, bat_w, dt);
            }
        }
        self.last_integrate = Some(now);
    }
}

fn current_date_string() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Simple UTC date formatting without chrono
    let days = secs / 86400;
    let (year, month, day) = days_to_ymd(days as i64);
    format!("{:04}-{:02}-{:02}", year, month, day)
}

// Gregorian calendar conversion (Howard Hinnant's algorithm)
fn days_to_ymd(days_since_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_epoch + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe/1460 + doe/36524 - doe/146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365*yoe + yoe/4 - yoe/100);
    let mp = (5*doy + 2) / 153;
    let d = doy - (153*mp + 2)/5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}
