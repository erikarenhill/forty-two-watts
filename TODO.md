# TODO — forty-two-watts 🐬

## Short term

- [ ] **Telemetry history in redb** — save readings every 5s, retain 3 days, serve via `GET /api/history`, chart loads data on page open instead of starting empty
- [ ] **Peak shaving mode** — configurable grid import ceiling (e.g. max 2000W import) instead of targeting 0W. New mode `peak_shaving` with `peak_limit_w` config
- [ ] **EV charging signal** — `POST /api/ev_charging {"power_w": 7000, "active": true}` tells the EMS a car is charging. Batteries should NOT discharge to cover EV load — only self-consume for house load. Also expose via HA MQTT command topic
- [ ] **Fix HA mode selector** — mode state format doesn't match HA options, shows "unknown". Ensure `self_consumption` string matches exactly
- [ ] **Compact UI** — fuse gauge visualization, per-battery weight/limit sliders, responsive layout that fits without scrolling
- [ ] **Per-battery config in UI** — SoC min/max limits, max charge/discharge power, priority weight — editable from the dashboard

## Medium term

- [ ] **More Lua drivers** — document how to add new devices (template in docs/lua-drivers.md), test with additional inverters/batteries
- [ ] **Systemd service** — auto-start on RPi boot, restart on crash, log to journald
- [ ] **MPC controller** — replace PI with Model Predictive Control (`clarabel` or `osqp` crate) for constraint-aware optimization that plans N steps ahead
- [ ] **Decouple measurement from control** — drivers push async telemetry with timestamps, control loop runs on fixed timer, Kalman provides best estimate at each tick
- [ ] **CI/CD** — GitHub Actions workflow: build static musl binaries for arm64+amd64 on tag push, auto-create release with artifacts
- [ ] **Load display stability** — improve Kalman filter tuning for load calculation, or compute load from dedicated measurement point

## Done

- [x] PI controller (Kp=0.4, Ki=0.05) with anti-windup
- [x] 1D Kalman filter per DER signal (auto-adaptive noise rejection)
- [x] Lua driver system (ferroamp.lua + sungrow.lua from srcful-device-support)
- [x] Ferroamp EnergyHub MQTT driver — charge/discharge/auto verified at 200W
- [x] Sungrow SH hybrid Modbus driver — charge/discharge verified, auto-configures discharge limit
- [x] Anti-oscillation: slew rate 300W/cycle, 10s command holdoff, 42W deadband
- [x] Fuse guard (16A shared breaker)
- [x] 5 dispatch modes: idle, self_consumption, charge, priority, weighted
- [x] REST API: status, mode, target, drivers, health
- [x] Web dashboard with real-time chart (grid, PV, load, per-battery actual+target)
- [x] Home Assistant MQTT autodiscovery (15 entities, mode selector, grid target slider)
- [x] redb state persistence for crash recovery
- [x] Static musl binaries (linux/arm64 + linux/amd64) via Docker
- [x] GitHub release v0.1.0 with downloadable binaries
- [x] Deployed to RPi (192.168.192.40)
- [x] Douglas Adams theme throughout 🐬
