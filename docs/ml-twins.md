# Digital twins — ML models that feed the planner

The MPC planner is only as good as its forecasts. Rather than relying
on generic physics formulas or flat baselines, we learn system-specific
behaviour online from the telemetry the stack already produces.

Three twins run continuously:

| Package | Learns | Feeds into |
|---|---|---|
| `internal/pvmodel` | PV array output vs. weather | MPC slot `pv_w` |
| `internal/loadmodel` | Household consumption vs. time-of-week | MPC slot `load_w` |
| `internal/priceforecast` | Spot prices vs. hour-of-week × month | MPC slots beyond day-ahead |

All three follow the same operating principles:

1. **Cold-start safely.** A reasonable prior is active on day 1 — no
   zeros, no flat means, no "wait a week". The MPC gets usable input
   immediately.
2. **Learn robustly.** Running means / EMAs, not unconstrained least
   squares. Outlier rejection. No silent drift.
3. **Stay explainable.** Parameters are interpretable (per-hour
   averages, per-degree heating gain, α coefficients), not a black
   box.
4. **Trust the signal you have.** A `trust` factor blends learned
   values with the prior based on sample count.

---

## PV digital twin (`pvmodel`)

### Model

Linear RLS (recursive least squares) over 7 features:

```
x = [ 1,
      clearsky_w,
      clearsky_w × (1 − cloud/100)^1.5,
      clearsky_w × sin(2π·h/24), clearsky_w × cos(2π·h/24),
      clearsky_w × sin(4π·h/24), clearsky_w × cos(4π·h/24) ]

pv_w = β · x
```

Captures:

- system gain (β₁ + β₂)
- cloud attenuation (the `(1−c)^1.5` factor is a heuristic the model can refine)
- orientation / shading through the first + second harmonic of hour-of-day
  — enough to fit a morning-shaded tree line or a west-facing panel
  that peaks after noon

### Cold start

`β = [0, 0, rated/1000, 0, 0, 0, 0]` reproduces the naive physics
formula `pv = rated × clearsky × cloud_factor`. Day-1 predictions are
identical to the pre-twin behaviour. `Predict()` blends learned β with
the naive prior by `trust = min(samples/50, 1)`, so RLS transient
swings can't produce wild forecasts in the first few updates.

### Update

Every 60 s the service samples current telemetry (summed across all
DerPV readings) and the current clear-sky + cloud. One RLS step with
forgetting factor 0.995 (≈200-sample effective window). Outliers (>10×
MAE) rejected once warmed up. Skips at night (`clearsky < 50 W/m²`).

### Persistence + quality

Model state JSON-stored under `config` key `pvmodel/state`. Restored
on boot — quality resumes where it left off.

`Quality()` returns 0..1 based on MAE relative to rated. 1.0 means MAE
≤ 5% of rated output.

### Config

```yaml
weather:
  pv_rated_w: 10000     # nameplate (W). Seeds the physics prior.
```

### API

- `GET /api/pvmodel` — model state (β, samples, MAE, quality, rated)
- `POST /api/pvmodel/reset` — clear and re-seed (use after panel changes)

### Verified performance

Synthetic 10 kW south-facing array with soft morning shading lobe,
92% of rated peak, 2% measurement noise:

- naive physics prior MAE: 567 W
- trained twin MAE after 30 days × 96 samples/day: 123 W
- **78% reduction** vs the naive formula

---

## Load digital twin (`loadmodel`)

### Model

168 EMA buckets (7 days × 24 hours of week) instead of RLS harmonics.
Every hour of the week has its own running mean. The direct
hour-of-week parameterisation is robust, interpretable, and converges
fast (one sample per bucket per week).

### Typical-home prior

Every bucket starts at a plausible Swedish-home value:

- 300 W overnight baseload
- morning peak ~2000 W at 07:00–08:00
- midday lobe ~600 W at 13:00
- evening peak ~2500 W at 18:30–19:00 (weekday) / 19:00 (weekend, ~15% lower)

Day-1 predictions are immediately sensible. The MPC gets useful `load_w`
per slot from the first plan.

### Update

Every 60 s:

```
measured_load_w = grid_w − pv_w − bat_w   (site sign)
bucket_idx      = HourOfWeek(now)
bucket.mean     ← EMA(bucket.mean, measured_load_w, α=0.1)
```

First 10 samples per bucket use exact running-mean (crisp initial
convergence), then switch to EMA (smooth drift as the home evolves).

### Trust-weighted prediction

```
trust = min(bucket.samples / MinTrustSamples, 1)   // MinTrustSamples = 8
prediction = trust × bucket.mean + (1 − trust) × typical_prior
```

### Heating correction (manual)

Indoor heating tracks strongly with outdoor temperature below ~18°C.
Rather than identify this from noisy online data, we expose it as a
user-configured scalar:

```yaml
weather:
  heating_w_per_degc: 300     # W per °C below 18°C (typical 200–500)
```

Prediction adds `heating_w_per_degc × max(18 − temp_c, 0)` on cold
slots. Temperature comes from the same met.no forecast cache.

### API

- `GET /api/loadmodel` — samples, MAE, heating coefficient, quality, warm-bucket count
- `POST /api/loadmodel/reset` — wipe, e.g. after appliance changes

### Verified performance

Synthetic 2-peak household with 3% measurement noise, 4 weeks training:

- flat-mean baseline MAE: 746 W
- trained twin MAE: 459 W
- **38% reduction**

(Lower than the PV twin's 78% because a flat baseline is already a
better starting point for load than for PV, where a flat value ignores
the dominant diurnal cycle.)

---

## Price forecaster (`priceforecast`)

### Why

Day-ahead auctions publish tomorrow's prices around 13:00 CET. Before
that, the MPC horizon used to silently truncate at "end of today" —
exactly when operators wanted overnight planning most.

### Model

Per-zone hour-of-week × month profile. 168 buckets holding EMA of raw
spot öre/kWh. 12 monthly multipliers for seasonality.

```
predict(t, zone) = bucket[weekday(t), hour(t)] × month_multiplier[month(t)]
```

### Baked-in cold-start prior

Without any fit, the model returns a typical Nordic shape:

- morning peak 07:00–09:00 (1.6× base)
- evening peak 17:00–20:00 (1.85× base)
- midday trough 11:00–14:00 (0.55× base, solar flood)
- overnight baseline 00:00–05:00 (0.65× base)
- weekend ~15% damped at peaks

Seasonal multipliers peak in January/December (1.35–1.40×) and bottom
in July (0.70×). Base level tuned per zone (SE3/SE4 ≈ 80 öre, SE1/SE2
≈ 50 öre, etc.).

Result: day-0 predictions already look like a real day. Real history
overrides the bake via `FitFromHistory`.

### Learning from history

Every 6 hours the service re-fits from the last 90 days of stored
prices. The fit is a simple hour-of-week × month average — no RLS, no
regularization. Good enough because this is a noise-reduction task, not
a market-timing task.

### Cold-start with your own history

Drop a CSV at `seed/prices.csv` next to the config:

```csv
zone,slot_ts_ms,slot_len_min,spot_ore_kwh
SE3,1704067200000,60,23.5
SE3,1704070800000,60,19.2
...
```

On boot, the service ingests (idempotent — UPSERT on `zone+ts`) and
refits. If your source is ENTSO-E EUR/MWh:
`spot_ore_kwh = (eur_per_mwh × fx_rate) / 10`.

Years of history make the forecaster markedly more useful for unusual
calendar events (e.g. holidays that stay cheap even if they fall on a
weekday).

### Confidence flag

Synthesized price rows are tagged `source="forecast"`. The MPC service
reads this in `buildSlots` and sets slot `confidence = 0.6`, which
causes the DP to blend the price toward the horizon mean (see
`docs/mpc-planner.md`).

### API

- `GET /api/prices` — real-only (from the elprisetjustnu/ENTSOE fetch)
- `GET /api/mpc/plan` — each action includes `price_ore + confidence`
  so the UI can distinguish real from predicted per slot

---

## Interactions with the MPC

Each 15-minute replan:

1. MPC pulls prices for `[now, now+48h]` from the store.
2. For slots past the day-ahead cutoff, `extendPricesWithForecast()`
   calls the forecaster and appends synthesized rows tagged
   `source="forecast"`.
3. `buildSlots()` iterates the combined row list, pulling `pv_w` from
   `pvmodel.Predict(t, cloud)` and `load_w` from `loadmodel.Predict(t)`.
   Slot `confidence` = 1.0 for real, 0.6 for forecast.
4. DP runs with confidence-blended effective prices.
5. Output plan has a `reason` string per slot for the UI hover.

---

## What we'd add next

- **Temperature-dependent load fit** — online identification rather
  than manual `heating_w_per_degc`. Requires separating
  heating-related variance from bucket baseline with a regularised
  two-variable regression. Left manual for now because noisy online
  fit did worse than operator-declared.
- **Per-resource battery twins** — ARX(1) already exists in
  `internal/battery`; next step is GP regression or a tiny NN for
  saturation curves + SoH decay. Tracked as task #46.
- **Price uncertainty quantification** — P10/P50/P90 per slot, not
  just a point estimate. Lets the MPC use CVaR for conservative
  scheduling when uncertainty is high.
