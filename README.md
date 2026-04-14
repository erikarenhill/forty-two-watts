# forty-two-watts 🐬

> *"The Answer to the Ultimate Question of Life, the Universe, and Grid Balancing is... 42 watts."*

Unified Home Energy Management System. Coordinates multiple batteries + PV +
grid + loads on a shared grid connection so they don't oscillate or blow the
main fuse.

Three layers in one binary:

1. **Inner control loop** (5 s) — PI + cascade + fuse + SoC clamps,
   executes grid-power targets no matter where they come from.
2. **MPC planner** (15 min) — dynamic programming over a discretized
   SoC grid, 48-hour horizon, three strategies (self-consumption /
   cheap charging / arbitrage), confidence-weighted when prices are
   ML-forecasted.
3. **Digital twins** (1 min) — online RLS / bucket models that learn
   the system's PV curve, household load profile, and spot-price
   pattern from its own telemetry. Feed slot-by-slot forecasts into
   the MPC.

**This branch (`go-port`)** is the current mainline: **Go + WASM drivers**
(via wazero). The previous Rust implementation lives on `master` and is
unchanged.

---

## Architecture in one sentence

A Go binary runs the control loop, the HTTP API, and the Home Assistant
bridge; WASM driver modules (one `.wasm` file per device type) do all
protocol work — MQTT, Modbus, JSON parsing, bit twiddling — inside a
capability-scoped sandbox with a tiny host ABI.

```
┌────────────────────────────────────────────────────────────────┐
│                   forty-two-watts (Go)                          │
│                                                                 │
│  ┌──────────┐  ┌──────────┐   WASM driver modules (wazero)     │
│  │ Ferroamp │  │ Sungrow  │   — fat. all protocol logic here.  │
│  │  .wasm   │  │  .wasm   │                                    │
│  └────┬─────┘  └────┬─────┘                                    │
│       │              │                                          │
│       ▼              ▼                                          │
│  ┌──────────────────────────┐                                  │
│  │    Telemetry store        │  (site sign convention —        │
│  │    + Kalman smoothing     │   + = import to site,           │
│  │    + driver health        │   PV − (generation),            │
│  └──────────────┬───────────┘   bat + charge / − discharge)    │
│                 │                                               │
│  ┌──────────────▼────────────┐  ┌────────────────────────┐    │
│  │  Control loop              │  │  HTTP API + web UI     │    │
│  │  PI + cascade + self-tune  │  │  :8080                 │    │
│  │  + ARX(1) RLS per battery  │  └────────────────────────┘    │
│  └──────────────┬────────────┘                                  │
│                 │                 ┌────────────────────────┐    │
│                 └────────────────▶│  HA MQTT bridge         │    │
│                                   │  (autodiscovery)        │    │
│                                   └────────────────────────┘    │
│  ┌──────────────────────────┐                                  │
│  │  SQLite state DB         │  config, events, battery models, │
│  │  (tiered history)        │  history hot/warm/cold tiers,    │
│  └──────────────────────────┘  prices, forecasts, twin state   │
│                                                                 │
│  ┌──────────────────────────┐   ┌────────────────────────┐     │
│  │  MPC planner (15 min)    │◀──│  Digital twins (1 min) │     │
│  │  • DP over SoC grid      │   │  • pvmodel (RLS)       │     │
│  │  • 48 h horizon          │   │  • loadmodel (buckets) │     │
│  │  • three strategies      │   │  • priceforecast       │     │
│  │  • confidence blending   │   │  • baked cold-start    │     │
│  │  • per-slot reasons      │   │    priors + auto-fit   │     │
│  └────────────┬─────────────┘   └────────────────────────┘     │
│               │  grid_target_w per slot                         │
│               └─────▶ consumed by the control loop above        │
└────────────────────────────────────────────────────────────────┘
```

Read the whole story: [`MIGRATION_PLAN.md`](MIGRATION_PLAN.md)
Sign convention (critical): [`docs/site-convention.md`](docs/site-convention.md)

## Quick start

Prereqs: Go 1.22+, Rust stable + `wasm32-wasip1` target (for building
driver modules), `make`.

```bash
# Build WASM drivers + Go binaries, run the full local stack with simulators
make dev

# Open the UI
open http://localhost:8080
```

That's it — no real hardware, no external MQTT broker needed. Two
simulators stand in for a Ferroamp EnergyHub (MQTT) and a Sungrow
SH10RT (Modbus TCP) with realistic first-order battery response.

## Features

- **PI controller** with anti-windup, 6 dispatch modes (idle,
  self_consumption, peak_shaving, charge, priority, weighted)
- **Cascade control** — per-battery inner PI tuned from the learned
  τ, plus inverse-gain compensation so commands actually land
- **Online learning** — ARX(1) model per battery via RLS with forgetting
  factor, capability-aware saturation curves, hardware-health drift
- **Self-tune** — 3-minute step-response calibration per battery to set
  a clean baseline; safety-gated by confidence
- **WASM drivers** — FAT drivers. The host provides only capabilities
  (MQTT, Modbus, time, logging). Each driver does its own protocol
  parsing, state management, command translation. No `host.decode_*`
  functions; everything lives inside the sandbox.
- **Hot-reload** — config.yaml + settings UI round-trip, file watcher
  applies changes live for 99% of settings
- **Tiered history** — 30d at 5s, 12mo at 15min buckets, forever at 1d
  buckets. Pure SQL aggregation (SQLite, no CGo).
- **Home Assistant MQTT** — autodiscovery publishes sensors for grid,
  PV, battery, load, SoC, per-driver + mode/target/peak/EV commands

## Deploy to a Raspberry Pi

```bash
# One-time: make sure rustup has wasm32-wasip1 installed
rustup target add wasm32-wasip1

# Build the release tarballs (arm64 + amd64)
make release

# Push a release via GitHub (if you want to deploy from CI later)
gh release create v1.0.0 release/*.tar.gz --generate-notes

# Or deploy directly
./scripts/deploy-go.sh homelab-rpi
```

## Library choices (all pure Go)

| Need | Choice | Why |
|---|---|---|
| WASM runtime | [wazero](https://wazero.io) | Zero CGo, zero deps, prod-ready |
| State DB | [modernc.org/sqlite](https://gitlab.com/cznic/sqlite) | Pure Go SQLite, SQL queries for history |
| MQTT client | eclipse/paho.mqtt.golang | Battle-tested |
| MQTT broker (tests/sim) | mochi-mqtt/server | Embeddable |
| Modbus TCP | simonvetter/modbus | Both client and server |
| File watcher | fsnotify/fsnotify | Cross-platform |
| YAML | gopkg.in/yaml.v3 | Standard |
| HTTP | stdlib `net/http` | Go 1.22+ method-scoped routing |
| Logging | stdlib `log/slog` | Structured, inbuilt |

Nothing exotic. Nothing that requires a C toolchain. One `go build`
produces a static binary that drops onto a Pi.

## Repo layout

```
forty-two-watts/
├── go/
│   ├── cmd/
│   │   ├── forty-two-watts/   # main binary
│   │   ├── sim-ferroamp/      # embedded MQTT broker + Ferroamp fake
│   │   └── sim-sungrow/       # Modbus TCP Sungrow fake
│   ├── internal/
│   │   ├── api/               # HTTP handlers
│   │   ├── battery/           # ARX(1) + RLS + cascade
│   │   ├── config/            # YAML + validation
│   │   ├── configreload/      # fsnotify watcher
│   │   ├── control/           # PI + dispatch modes + fuse guard
│   │   ├── drivers/           # wazero runtime + registry + host ABI
│   │   ├── ha/                # Home Assistant MQTT bridge
│   │   ├── mqtt/              # paho wrapper
│   │   ├── modbus/            # simonvetter wrapper
│   │   ├── selftune/          # step-response calibration
│   │   ├── state/             # SQLite + tiered history
│   │   └── telemetry/         # DER store + Kalman + health
│   └── test/e2e/              # full-stack integration test
├── wasm-drivers/
│   ├── ferroamp/              # Rust → wasm32-wasip1
│   └── sungrow/               #     (~280 LOC each)
├── drivers-wasm/              # compiled .wasm modules (gitignored)
├── web/                       # static UI (HTML/CSS/JS)
├── docs/                      # architecture docs
├── config.example.yaml        # sample config
├── Makefile                   # build orchestration
└── MIGRATION_PLAN.md          # full rationale for the Go + WASM port
```

## Testing

```bash
make test        # all unit + integration tests (Go + Rust)
make e2e         # full-stack end-to-end test (simulates real hardware)
```

The e2e test stands up both simulators, loads the compiled WASM drivers,
runs the control loop, and verifies:
- Drivers load, initialize, emit telemetry
- Site sign convention holds (PV −, grid + for import, bat + for charge)
- Control loop responds to grid-target changes in the correct direction
- Mode switching persists through the state DB
- Battery models accumulate samples through RLS
- Per-model reset wipes them cleanly
- Settings round-trip through the HTTP API

## Documentation

- [`docs/site-convention.md`](docs/site-convention.md) — THE sign convention, enforced at driver boundary
- [`docs/battery-models.md`](docs/battery-models.md) — ARX(1), RLS, cascade, self-tune
- [`docs/clamping.md`](docs/clamping.md) — the seven clamps and why each matters
- [`docs/configuration.md`](docs/configuration.md) — full YAML schema reference
- [`docs/mpc-planner.md`](docs/mpc-planner.md) — MPC strategies, confidence blending, decision reasons
- [`docs/ml-twins.md`](docs/ml-twins.md) — PV + load + price digital twins
- [`docs/ha-integration.md`](docs/ha-integration.md) — Home Assistant MQTT bridge
- [`docs/host-api.md`](docs/host-api.md) — WASM driver ABI
- [`docs/lua-drivers.md`](docs/lua-drivers.md) — legacy Lua driver format
- [`MIGRATION_PLAN.md`](MIGRATION_PLAN.md) — why Go + WASM, library evaluations

---

*So long, and thanks for all the watts.* 🐬
