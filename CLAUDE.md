# forty-two-watts — project orientation

Unified Home Energy Management System. This branch (`go-port`) is the
Go + WASM driver implementation. Master still has the original Rust port.

## Mental model

**Site sign convention**: positive W = energy flowing INTO the site across
the grid-meter boundary. Grid import (+), PV generation (−), battery
charge (+ as load), battery discharge (−). The driver layer is the ONLY
place sign conversion happens — above it, every layer uses the site
convention. Read `docs/site-convention.md` before touching any power-math
code.

**FAT drivers**: WASM modules do all protocol work. Host exposes only
capabilities (MQTT, Modbus, time, log). No `decode_u32_le` or other
protocol helpers in the host. Drivers use real libraries (serde_json,
etc.) inside the sandbox.

**Clamping discipline**: every clamp must protect against a *quantifiable
risk*. Read `docs/clamping.md` for the seven current clamps and the
saturation-curve feedback-loop bug we shipped then fixed.

## Key packages

| Package | Purpose |
|---|---|
| `go/internal/config` | YAML config + validation + atomic save |
| `go/internal/state` | SQLite persistence + tiered history |
| `go/internal/telemetry` | DerStore with Kalman per signal + driver health |
| `go/internal/control` | PI + dispatch modes + slew + fuse guard |
| `go/internal/battery` | ARX(1) model + RLS + cascade + saturation curves |
| `go/internal/selftune` | Step-response state machine + fitter |
| `go/internal/drivers` | wazero runtime + ABI + registry |
| `go/internal/api` | HTTP endpoints (Go 1.22+ method mux) |
| `go/internal/configreload` | fsnotify watcher + reload dispatch |
| `go/internal/ha` | Home Assistant MQTT autodiscovery + bridge |
| `go/internal/mqtt` | paho client wrapper implementing drivers.MQTTCap |
| `go/internal/modbus` | simonvetter wrapper implementing drivers.ModbusCap |
| `wasm-drivers/ferroamp` | Rust → wasm32-wasip1 Ferroamp driver |
| `wasm-drivers/sungrow` | Rust → wasm32-wasip1 Sungrow driver |
| `go/test/e2e` | Full-stack test: sims + main + WASM drivers + HTTP |

## Building & testing

```bash
make wasm         # compile .wasm drivers (needs wasm32-wasip1 Rust target)
make test         # unit + integration tests
make e2e          # full-stack end-to-end test
make dev          # start sims + main app locally
make build-arm64  # cross-compile for RPi
make release      # tarballs for deploy
```

No CGo anywhere — pure Go + Rust → WASM. `go build` produces a static
single-binary distribution.

## Adding a new driver

1. Copy `wasm-drivers/ferroamp/` as a template into `wasm-drivers/mydevice/`
2. Implement `driver_init`, `driver_poll`, `driver_command`, `driver_default`,
   `driver_cleanup` in `src/lib.rs`. Use `host::` helpers for I/O.
3. Add `"mydevice"` to `WASM_DRIVERS` in the Makefile
4. Add an entry to `config.yaml` with the appropriate `capabilities:` block
5. Driver starts on next restart (or hot-reload via file watcher)

The host ABI is stable across drivers — see
`go/internal/drivers/abi.go` for the contract.

## WASM ABI

- Driver EXPORTS: `wasm_alloc`, `wasm_dealloc`, `driver_init`,
  `driver_poll`, `driver_command`, `driver_default`, `driver_cleanup`
- Host IMPORTS (under `"host"` namespace): `log`, `millis`,
  `set_poll_interval`, `emit_telemetry`, `set_sn`, `set_make`,
  `mqtt_{subscribe,publish,poll_messages}`, `modbus_{read,write_single,write_multi}`
- All string / byte-slice parameters use `(ptr: i32, len: i32)` pairs
  into driver memory. Driver allocates its own memory via `wasm_alloc`
  so the host can copy strings into it.

MQTT / Modbus functions return `ErrNoCapability` via status codes if
the driver wasn't granted the relevant capability in config.

## Code conventions

- `slog` for all logging
- Explicit mutexes — no atomic tricks unless measurably needed
- SQLite queries in `internal/state/store.go`, nothing embedded elsewhere
- Driver code in Rust, not Go — sandbox guarantees + ecosystem libraries
- Tests colocated with code, `_test.go` files
- Integration tests in `go/test/e2e/` (separate package to keep public
  and internal concerns cleanly split)

## When things look weird

- **Sign is wrong somewhere**: it's ALWAYS a bug at the driver boundary.
  Above the driver layer is always site convention.
- **Battery drifting from target**: check confidence. Below 0.5 the
  cascade bypasses the inverse model (gates on confidence intentionally).
- **History queries slow**: check `idx_hot_ts` is there; SQL uses range
  scans. `Prune()` should be running periodically to age data to warm/cold.
- **Tests fail with `drivers-wasm/*.wasm not found`**: run `make wasm`
  first. CI should always do `make wasm` before `make test`.

## Docs for operators + devs

- `docs/site-convention.md` — sign convention (must-read)
- `docs/battery-models.md` — ARX(1), RLS, self-tune
- `docs/clamping.md` — the safety clamps
- `docs/configuration.md` — YAML schema
- `MIGRATION_PLAN.md` — why Go + WASM (for historical context)
