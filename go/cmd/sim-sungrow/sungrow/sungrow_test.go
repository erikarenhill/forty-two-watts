package sungrow

import (
	"math"
	"testing"
	"time"
)

func approx(t *testing.T, got, want, tol float64, label string) {
	t.Helper()
	if math.Abs(got-want) > tol {
		t.Errorf("%s: got %.3f want %.3f tol %.3f", label, got, want, tol)
	}
}

// ---- Physics tests ----

func TestChargeCommand(t *testing.T) {
	cfg := Default()
	cfg.ResponseTauS = 0.1
	cfg.CapacityWh = 1e9 // effectively infinite
	s := New(cfg)
	s.SetMode(ModeForced)
	s.SetForceCmd(ForceCharge)
	s.SetForceW(2000)
	for i := 0; i < 20; i++ {
		s.Tick(100 * time.Millisecond)
	}
	snap := s.Tick(100 * time.Millisecond)
	// Steady-state with gain 0.96 → ~1920W
	if snap.ActualBatW < 1500 || snap.ActualBatW > 2100 {
		t.Errorf("charge steady-state should be ~1920W, got %.0f", snap.ActualBatW)
	}
}

func TestDischargeCommand(t *testing.T) {
	cfg := Default()
	cfg.ResponseTauS = 0.1
	cfg.CapacityWh = 1e9
	s := New(cfg)
	s.SetMode(ModeForced)
	s.SetForceCmd(ForceDischarge)
	s.SetForceW(1500)
	for i := 0; i < 20; i++ {
		s.Tick(100 * time.Millisecond)
	}
	snap := s.Tick(100 * time.Millisecond)
	// Steady-state with gain 0.96 → ~-1440W
	if snap.ActualBatW > -1200 || snap.ActualBatW < -1600 {
		t.Errorf("discharge steady-state should be ~-1440W, got %.0f", snap.ActualBatW)
	}
}

func TestStopModeIsIdle(t *testing.T) {
	cfg := Default()
	cfg.ResponseTauS = 0.1
	s := New(cfg)
	// First charge
	s.SetMode(ModeForced)
	s.SetForceCmd(ForceCharge)
	s.SetForceW(2000)
	for i := 0; i < 10; i++ {
		s.Tick(100 * time.Millisecond)
	}
	// Switch to stop
	s.SetMode(ModeStop)
	for i := 0; i < 20; i++ {
		s.Tick(100 * time.Millisecond)
	}
	snap := s.Tick(0)
	if math.Abs(snap.ActualBatW) > 100 {
		t.Errorf("ModeStop should relax actual toward 0, got %.0f", snap.ActualBatW)
	}
}

func TestSoCDeratingAboveHighSoC(t *testing.T) {
	cfg := Default()
	cfg.SoC = 0.98
	cfg.ResponseTauS = 0.01
	s := New(cfg)
	s.SetMode(ModeForced)
	s.SetForceCmd(ForceCharge)
	s.SetForceW(5000)
	s.Tick(time.Second)
	snap := s.Tick(time.Second)
	// At 98% SoC, max charge = 5000 * (1-0.98)/0.05 = 2000W
	// Actual ≤ 2000 * 0.96 ≈ 1920W
	if snap.ActualBatW > 2200 {
		t.Errorf("charge should derate near 98%% SoC, got %.0f", snap.ActualBatW)
	}
}

func TestWritableMaxChargeLimit(t *testing.T) {
	s := New(Default())
	s.SetMaxChargeW(1500) // externally lowered via reg 33046
	s.SetMode(ModeForced)
	s.SetForceCmd(ForceCharge)
	s.SetForceW(5000)
	for i := 0; i < 20; i++ {
		s.Tick(100 * time.Millisecond)
	}
	snap := s.Tick(0)
	if snap.ActualBatW > 1600 {
		t.Errorf("should respect max_charge=1500, got %.0f", snap.ActualBatW)
	}
}

// ---- Register encoding tests ----

func TestRegisterEncoding_BatteryBlock(t *testing.T) {
	cfg := Default()
	cfg.SoC = 0.45
	s := New(cfg)
	s.SetMode(ModeForced)
	s.SetForceCmd(ForceDischarge)
	s.SetForceW(1000)
	// Let it converge
	for i := 0; i < 50; i++ {
		s.Tick(100 * time.Millisecond)
	}
	snap := s.Tick(0)
	inp, _ := EncodeSnapshot(snap, cfg)

	// 13019 = voltage ×0.1V → expect 480 (48.0V)
	if inp[13019] != 480 {
		t.Errorf("reg 13019 (bat V): expected 480, got %d", inp[13019])
	}
	// 13021 = abs(battery W) unsigned
	batMag := uint16(math.Abs(snap.ActualBatW))
	if inp[13021] != batMag {
		t.Errorf("reg 13021 (bat |W|): expected ~%d, got %d", batMag, inp[13021])
	}
	// 13022 = SoC × 1000 → for SoC ~ 0.45, should be ~450
	if inp[13022] < 400 || inp[13022] > 500 {
		t.Errorf("reg 13022 (SoC×1000): expected ~450, got %d", inp[13022])
	}
	// 13000 bit 2 (0x0004) set when discharging
	if inp[13000]&0x0004 == 0 {
		t.Errorf("reg 13000: bit 2 (discharging) should be set, got 0x%04x", inp[13000])
	}
}

func TestRegisterEncoding_U32LE(t *testing.T) {
	inp, _ := EncodeSnapshot(Snapshot{
		PVWh:     1_234_567, // Wh → ×0.1 kWh → 12345
		ImportWh: 50_000,    // → 500
	}, Default())
	// 13002 low word, 13003 high word of 12345
	low := inp[13002]
	high := inp[13003]
	combined := uint32(high)<<16 | uint32(low)
	if combined != 12345 {
		t.Errorf("13002-13003 U32 LE: expected 12345, got %d (low=%d high=%d)", combined, low, high)
	}
	low = inp[13036]
	high = inp[13037]
	combined = uint32(high)<<16 | uint32(low)
	if combined != 500 {
		t.Errorf("13036-13037 U32 LE: expected 500, got %d", combined)
	}
}

func TestRegisterEncoding_I32LENegative(t *testing.T) {
	inp, _ := EncodeSnapshot(Snapshot{GridW: -1500}, Default())
	// 5600-5601 I32 LE: -1500 = 0xFFFFFA24, low=0xFA24, high=0xFFFF
	low := inp[5600]
	high := inp[5601]
	combined := int32(uint32(high)<<16 | uint32(low))
	if combined != -1500 {
		t.Errorf("5600-5601 I32 LE: expected -1500, got %d", combined)
	}
}

func TestRegisterEncoding_SerialNumber(t *testing.T) {
	cfg := Default()
	cfg.SerialNumber = "ABCDEFGHIJ" // 10 chars
	inp, _ := EncodeSnapshot(Snapshot{}, cfg)
	// 4990-4999 carries 10 chars as 5 U16 regs (2 chars each), but map says 10 regs × 2 chars = 20 chars
	// Re-reading: actually our encoder puts 2 chars per reg over 10 regs = 20 char capacity.
	// For "ABCDEFGHIJ" we expect first reg = 'A'<<8 | 'B', etc.
	want := uint16('A')<<8 | uint16('B')
	if inp[4990] != want {
		t.Errorf("reg 4990 ASCII: expected 0x%04x, got 0x%04x", want, inp[4990])
	}
}

func TestRegisterEncoding_DeviceType(t *testing.T) {
	cfg := Default()
	cfg.DeviceType = 3598
	inp, _ := EncodeSnapshot(Snapshot{}, cfg)
	if inp[4999] != 3598 {
		t.Errorf("reg 4999 devtype: expected 3598, got %d", inp[4999])
	}
}

// ---- RegisterBank (integration of sim + registers) ----

func TestBankWritePropagatesToSim(t *testing.T) {
	cfg := Default()
	cfg.ResponseTauS = 0.2 // fast convergence for test
	s := New(cfg)
	bank := NewRegisterBank(s)
	// Simulate the controller issuing a charge command
	if err := bank.WriteHolding(13051, []uint16{1500}); err != nil {
		t.Fatal(err)
	}
	if err := bank.WriteHolding(13050, []uint16{0xAA}); err != nil {
		t.Fatal(err)
	}
	if err := bank.WriteHolding(13049, []uint16{2}); err != nil {
		t.Fatal(err)
	}

	// With τ=0.2s, steady state (1500 × 0.96 = 1440W) is essentially reached in ~1s
	for i := 0; i < 30; i++ {
		s.Tick(100 * time.Millisecond)
	}
	snap := s.Tick(0)
	if snap.ActualBatW < 1200 {
		t.Errorf("expected charging (~1440W) after register writes, got %.0f", snap.ActualBatW)
	}
}

func TestBankReadAfterRefresh(t *testing.T) {
	s := New(Default())
	bank := NewRegisterBank(s)
	snap := s.Tick(time.Second)
	bank.Refresh(snap)

	// Read device type
	regs := bank.ReadInput(4999, 1)
	if regs[0] != s.Config().DeviceType {
		t.Errorf("device type read: got %d want %d", regs[0], s.Config().DeviceType)
	}

	// Read SoC (in 13022, ×1000)
	regs = bank.ReadInput(13019, 4)
	soc := float64(regs[3]) / 1000.0
	approx(t, soc, snap.SoC, 0.01, "SoC roundtrip through reg 13022")
}

func TestBankUnknownAddressReturnsZero(t *testing.T) {
	s := New(Default())
	bank := NewRegisterBank(s)
	bank.Refresh(s.Tick(time.Second))
	regs := bank.ReadInput(9999, 3)
	for i, r := range regs {
		if r != 0 {
			t.Errorf("unknown addr %d should read 0, got %d", 9999+i, r)
		}
	}
}
