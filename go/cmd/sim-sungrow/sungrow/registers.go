package sungrow

import (
	"errors"
	"sync"
)

// Register-map encoders follow the SH-series spec. All multi-register values
// are LITTLE-ENDIAN (low word at lower address, opposite of standard
// Modbus big-endian).

// EncodeSnapshot returns a map of register-address → value for both tables.
// Call after every Tick() to refresh the readable registers. The Modbus
// handler reads from these maps; writes mutate the Simulator.
func EncodeSnapshot(s Snapshot, cfg Config) (input, holding map[uint16]uint16) {
	input = make(map[uint16]uint16, 80)
	holding = make(map[uint16]uint16, 16)

	// ---- Input registers (read-only telemetry) ----

	// 4990-4999: 10-char serial number, ASCII in U16 regs (2 chars per reg, big-endian within reg)
	sn := cfg.SerialNumber
	for i := 0; i < 10; i++ {
		var hi, lo byte
		if 2*i < len(sn) { hi = sn[2*i] }
		if 2*i+1 < len(sn) { lo = sn[2*i+1] }
		input[4990+uint16(i)] = uint16(hi)<<8 | uint16(lo)
	}

	// 4999: Device type code
	input[4999] = cfg.DeviceType

	// 5000: Rated power (W)
	input[5000] = cfg.RatedW

	// 5007: Heatsink temperature, I16 × 0.1 C
	input[5007] = encI16(350) // 35.0 C, constant — enough for sim

	// 5010-5013: MPPT1/2 V (×0.1) and A (×0.1)
	input[5010] = uint16(400 * 10) // V
	input[5011] = uint16(max0(s.PVW/2/400) * 10)
	input[5012] = uint16(400 * 10)
	input[5013] = uint16(max0(s.PVW/2/400) * 10)

	// 5016-5017: PV power (U32 LE, W)
	putU32LE(input, 5016, uint32(max0(s.PVW)))

	// 5241: Grid frequency (×0.01 Hz)
	input[5241] = 5000

	// 5600-5601: Grid meter total power (I32 LE)
	putI32LE(input, 5600, int32(s.GridW))

	// 5602-5607: Per-phase meter power (3× I32 LE)
	each := int32(s.GridW / 3)
	putI32LE(input, 5602, each)
	putI32LE(input, 5604, each)
	putI32LE(input, 5606, each)

	// 5740-5742: per-phase meter voltage (×0.1 V)
	input[5740] = 2300
	input[5741] = 2300
	input[5742] = 2300

	// 5743-5745: per-phase meter current (×0.01 A)
	iPerPhase := uint16(absf(s.GridW) / 3 / 230 * 100)
	input[5743] = iPerPhase
	input[5744] = iPerPhase
	input[5745] = iPerPhase

	// 13000: Running status bits
	//   bit 1 (0x0002) = charging
	//   bit 2 (0x0004) = discharging
	var status uint16
	if s.ActualBatW > 10 { status |= 0x0002 }
	if s.ActualBatW < -10 { status |= 0x0004 }
	input[13000] = status

	// 13002-13003: PV lifetime energy (U32 LE × 0.1 kWh)
	putU32LE(input, 13002, uint32(s.PVWh/100))

	// 13019-13022: battery {V×0.1, A×0.1, W raw, SoC×0.1%}
	input[13019] = uint16(480) // 48.0 V × 10
	input[13020] = uint16(absf(s.ActualBatW) / 48 * 10)
	// Sungrow reports abs(bat_w) — direction encoded in 13000 status
	input[13021] = uint16(absf(s.ActualBatW))
	input[13022] = uint16(s.SoC * 1000) // 0..1000 (0.1%)

	// 13026-13027: battery discharge lifetime (U32 LE × 0.1 kWh)
	putU32LE(input, 13026, uint32(s.BatDischargeWh/100))

	// 13036-13037: meter import energy (U32 LE × 0.1 kWh)
	putU32LE(input, 13036, uint32(s.ImportWh/100))

	// 13040-13041: battery charge lifetime (U32 LE × 0.1 kWh)
	putU32LE(input, 13040, uint32(s.BatChargeWh/100))

	// 13045-13046: meter export energy (U32 LE × 0.1 kWh)
	putU32LE(input, 13045, uint32(s.ExportWh/100))

	// ---- Holding registers (read/write config + control) ----

	holding[13049] = uint16(s.Mode)
	holding[13050] = uint16(s.ForceCmd)
	holding[13051] = uint16(s.ForceW)

	// 13057-13058: SoC max/min (×0.1%)
	holding[13057] = uint16(s.SocMaxPct * 10)
	holding[13058] = uint16(s.SocMinPct * 10)

	// 33046/33047: max charge/discharge power (×0.01 kW, i.e. ×10 W)
	holding[33046] = uint16(s.MaxChargeW / 10)
	holding[33047] = uint16(s.MaxDischargeW / 10)

	return input, holding
}

// ---- Encoding helpers ----

func encI16(v int16) uint16 {
	if v < 0 { return uint16(int32(v) + 65536) }
	return uint16(v)
}

func putU32LE(m map[uint16]uint16, addr uint16, v uint32) {
	// Sungrow convention: low word at LOWER address
	m[addr]   = uint16(v & 0xFFFF)
	m[addr+1] = uint16(v >> 16)
}

func putI32LE(m map[uint16]uint16, addr uint16, v int32) {
	u := uint32(v)
	putU32LE(m, addr, u)
}

func max0(x float64) float64 {
	if x < 0 { return 0 }
	return x
}

func absf(x float64) float64 {
	if x < 0 { return -x }
	return x
}

// ---- RegisterBank: thread-safe view for the Modbus server ----

// ErrIllegalAddress is returned when a read/write targets an unmapped address.
var ErrIllegalAddress = errors.New("illegal register address")

// RegisterBank holds the last-encoded register values and lets the Modbus
// handler serve reads + dispatch writes back to the Simulator.
type RegisterBank struct {
	mu      sync.RWMutex
	input   map[uint16]uint16
	holding map[uint16]uint16
	sim     *Simulator
}

func NewRegisterBank(sim *Simulator) *RegisterBank {
	return &RegisterBank{
		sim:     sim,
		input:   make(map[uint16]uint16),
		holding: make(map[uint16]uint16),
	}
}

// Refresh replaces the register snapshot with values from the latest Tick.
func (r *RegisterBank) Refresh(snap Snapshot) {
	inp, hold := EncodeSnapshot(snap, r.sim.Config())
	r.mu.Lock()
	r.input = inp
	r.holding = hold
	r.mu.Unlock()
}

// ReadInput returns `count` consecutive input registers starting at addr.
// Unmapped addresses return 0 (real Sungrow inverters do the same rather than
// erroring for unknown addresses within the documented range).
func (r *RegisterBank) ReadInput(addr, count uint16) []uint16 {
	r.mu.RLock(); defer r.mu.RUnlock()
	out := make([]uint16, count)
	for i := uint16(0); i < count; i++ {
		out[i] = r.input[addr+i]
	}
	return out
}

// ReadHolding returns `count` consecutive holding registers starting at addr.
func (r *RegisterBank) ReadHolding(addr, count uint16) []uint16 {
	r.mu.RLock(); defer r.mu.RUnlock()
	out := make([]uint16, count)
	for i := uint16(0); i < count; i++ {
		out[i] = r.holding[addr+i]
	}
	return out
}

// WriteHolding handles a write-multiple-registers request. For each written
// address we dispatch to the Simulator methods so state actually updates.
// Writes to addresses we don't care about are silently accepted (parity with
// real inverter behavior).
func (r *RegisterBank) WriteHolding(addr uint16, values []uint16) error {
	for i, v := range values {
		a := addr + uint16(i)
		switch a {
		case 13049:
			r.sim.SetMode(Mode(v))
		case 13050:
			r.sim.SetForceCmd(ForceCmd(v))
		case 13051:
			r.sim.SetForceW(float64(v))
		case 33046:
			// Reg value is ×0.01 kW → W
			r.sim.SetMaxChargeW(float64(v) * 10)
		case 33047:
			r.sim.SetMaxDischargeW(float64(v) * 10)
		}
		// Also reflect in bank so subsequent reads see the write
		r.mu.Lock()
		r.holding[a] = v
		r.mu.Unlock()
	}
	return nil
}
