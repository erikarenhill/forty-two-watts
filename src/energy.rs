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

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, eps: f64) {
        assert!((a - b).abs() < eps, "expected {} ≈ {} (eps {})", a, b, eps);
    }

    #[test]
    fn integrate_import_one_hour() {
        // 1000W for 3600s = 1000Wh import
        let mut c = EnergyCounters::default();
        c.integrate(1000.0, 0.0, 0.0, 3600.0);
        approx(c.import_wh, 1000.0, 0.01);
        approx(c.export_wh, 0.0, 0.01);
    }

    #[test]
    fn integrate_export() {
        // -2000W (export) for 1800s = 1000Wh export
        let mut c = EnergyCounters::default();
        c.integrate(-2000.0, 0.0, 0.0, 1800.0);
        approx(c.export_wh, 1000.0, 0.01);
        approx(c.import_wh, 0.0, 0.01);
    }

    #[test]
    fn integrate_pv_uses_absolute() {
        let mut c = EnergyCounters::default();
        c.integrate(0.0, -3000.0, 0.0, 3600.0);
        approx(c.pv_wh, 3000.0, 0.01);
        // Same as positive — PV magnitude regardless of sign convention
        let mut c2 = EnergyCounters::default();
        c2.integrate(0.0, 3000.0, 0.0, 3600.0);
        approx(c2.pv_wh, 3000.0, 0.01);
    }

    #[test]
    fn integrate_battery_charge_vs_discharge() {
        let mut c = EnergyCounters::default();
        c.integrate(0.0, 0.0, 1500.0, 3600.0);    // charging
        c.integrate(0.0, 0.0, -1000.0, 1800.0);   // discharging 30 min
        approx(c.bat_charged_wh, 1500.0, 0.01);
        approx(c.bat_discharged_wh, 500.0, 0.01);
    }

    #[test]
    fn integrate_load_balance() {
        // Grid 500W import, PV 0, battery 0 → load = 500W
        // 1 hour → 500Wh load
        let mut c = EnergyCounters::default();
        c.integrate(500.0, 0.0, 0.0, 3600.0);
        approx(c.load_wh, 500.0, 0.01);

        // Grid 200W, PV -1500W (gen), battery 800W charging
        // Load = 200 - (-1500) - 800 = 900W → 900Wh in 1h
        let mut c2 = EnergyCounters::default();
        c2.integrate(200.0, -1500.0, 800.0, 3600.0);
        approx(c2.load_wh, 900.0, 0.01);
    }

    #[test]
    fn load_clamped_to_non_negative() {
        // Energy balance can momentarily go negative due to timing —
        // load can never be negative in physical reality.
        let mut c = EnergyCounters::default();
        c.integrate(-100.0, -1000.0, 500.0, 3600.0);
        // raw = -100 - (-1000) - 500 = 400 ✓
        approx(c.load_wh, 400.0, 0.01);

        let mut c2 = EnergyCounters::default();
        c2.integrate(0.0, -1000.0, -500.0, 3600.0);
        // raw = 0 - (-1000) - (-500) = 1500 ✓ all positive
        approx(c2.load_wh, 1500.0, 0.01);

        let mut c3 = EnergyCounters::default();
        // Construct a clearly-negative case
        c3.integrate(-2000.0, 1000.0, 500.0, 3600.0);
        // raw = -2000 - 1000 - 500 = -3500 → clamped to 0
        approx(c3.load_wh, 0.0, 0.01);
    }

    #[test]
    fn state_yaml_roundtrip_with_defaults() {
        // EnergyState should round-trip through serde, including new fields
        let mut s = EnergyState::default();
        s.today.import_wh = 1234.5;
        s.today.load_wh = 456.7;
        s.today_date = "2026-04-13".into();
        s.total.pv_wh = 99999.0;

        let json = serde_json::to_string(&s).unwrap();
        let back: EnergyState = serde_json::from_str(&json).unwrap();
        approx(back.today.import_wh, 1234.5, 0.001);
        approx(back.today.load_wh, 456.7, 0.001);
        approx(back.total.pv_wh, 99999.0, 0.001);
        assert_eq!(back.today_date, "2026-04-13");
    }

    #[test]
    fn old_state_without_load_wh_deserializes_with_default() {
        // Older state files (pre-load tracking) must load cleanly with load_wh=0
        let old_json = r#"{"today":{"import_wh":100,"export_wh":50,"pv_wh":200,"bat_charged_wh":30,"bat_discharged_wh":20},"total":{"import_wh":1000,"export_wh":500,"pv_wh":2000,"bat_charged_wh":300,"bat_discharged_wh":200},"today_date":"2026-04-12"}"#;
        let parsed: EnergyState = serde_json::from_str(old_json).expect("old format must parse");
        approx(parsed.today.import_wh, 100.0, 0.01);
        approx(parsed.today.load_wh, 0.0, 0.01); // missing field defaults
    }

    #[test]
    fn state_default_is_empty() {
        let s = EnergyState::default();
        assert_eq!(s.today.import_wh, 0.0);
        assert!(s.today_date.is_empty());
    }

    #[test]
    fn accumulator_skips_first_call() {
        // First call has no last_integrate, so nothing is added
        let mut acc = EnergyAccumulator::new(EnergyState::default());
        acc.integrate(1000.0, 0.0, 0.0);
        assert_eq!(acc.state.today.import_wh, 0.0);
    }

    #[test]
    fn accumulator_integrates_on_subsequent_calls() {
        let mut acc = EnergyAccumulator::new(EnergyState::default());
        acc.integrate(1000.0, 0.0, 0.0);
        std::thread::sleep(std::time::Duration::from_millis(100));
        acc.integrate(1000.0, 0.0, 0.0);
        // ~0.1s of 1000W ≈ 0.0278 Wh
        assert!(acc.state.today.import_wh > 0.02 && acc.state.today.import_wh < 0.05,
            "unexpected accumulation: {}", acc.state.today.import_wh);
    }

    #[test]
    fn day_rollover_resets_today_keeps_total() {
        let mut s = EnergyState::default();
        s.today_date = "1970-01-01".into(); // ancient — definitely not today
        s.today.import_wh = 5000.0;
        s.total.import_wh = 50000.0;
        s.check_day_rollover();
        assert_eq!(s.today.import_wh, 0.0); // reset
        assert_eq!(s.total.import_wh, 50000.0); // preserved
        assert_ne!(s.today_date, "1970-01-01"); // updated
    }

    #[test]
    fn day_rollover_no_op_when_same_date() {
        let today = current_date_string();
        let mut s = EnergyState::default();
        s.today_date = today.clone();
        s.today.import_wh = 100.0;
        s.check_day_rollover();
        assert_eq!(s.today.import_wh, 100.0); // preserved
        assert_eq!(s.today_date, today);
    }

    #[test]
    fn ymd_known_dates() {
        // 2026-04-13 is days_since_epoch where 1970-01-01 is day 0
        // 2000-01-01 = 10957 days since epoch
        let (y, m, d) = days_to_ymd(10957);
        assert_eq!((y, m, d), (2000, 1, 1));
        let (y, m, d) = days_to_ymd(0);
        assert_eq!((y, m, d), (1970, 1, 1));
    }
}
