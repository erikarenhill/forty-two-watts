package ferroamp

import (
	"math"
	"testing"
	"time"
)

func approx(t *testing.T, got, want, tol float64, label string) {
	t.Helper()
	if math.Abs(got-want) > tol {
		t.Errorf("%s: got %.3f, want %.3f (tol %.3f)", label, got, want, tol)
	}
}

func TestDefaultsSanity(t *testing.T) {
	s := New(Default())
	snap := s.Tick(time.Second)
	approx(t, snap.SoC, 0.5, 0.001, "initial soc")
	if snap.ActualBatW != 0 {
		t.Errorf("expected 0 actual bat W at start, got %f", snap.ActualBatW)
	}
}

func TestChargeIncreasesSoC(t *testing.T) {
	cfg := Default()
	cfg.ResponseTauS = 0.01 // essentially instantaneous
	cfg.CapacityWh = 1000   // small so SoC moves fast
	s := New(cfg)
	s.SetMode(ModeCharge, 1000)
	// Tick 10s at 10 intervals
	for i := 0; i < 10; i++ {
		s.Tick(time.Second)
	}
	snap := s.Tick(time.Second)
	// Charged ~11 * 1000W * (1s/3600) ≈ 3 Wh → 0.3% SoC change from 50% → ~50.3%
	if snap.SoC <= 0.5 {
		t.Errorf("charge should raise SoC above 0.5, got %.4f", snap.SoC)
	}
	if snap.ActualBatW <= 0 {
		t.Errorf("charging actual should be positive, got %f", snap.ActualBatW)
	}
}

func TestDischargeDecreasesSoC(t *testing.T) {
	cfg := Default()
	cfg.ResponseTauS = 0.01
	cfg.CapacityWh = 1000
	s := New(cfg)
	s.SetMode(ModeDischarge, 1000)
	for i := 0; i < 10; i++ {
		s.Tick(time.Second)
	}
	snap := s.Tick(time.Second)
	if snap.SoC >= 0.5 {
		t.Errorf("discharge should lower SoC below 0.5, got %.4f", snap.SoC)
	}
	if snap.ActualBatW >= 0 {
		t.Errorf("discharging actual should be negative, got %f", snap.ActualBatW)
	}
}

func TestFirstOrderLag(t *testing.T) {
	cfg := Default()
	cfg.ResponseTauS = 2.0
	cfg.CapacityWh = 1e9 // huge → SoC ignores, focus on response
	s := New(cfg)
	s.SetMode(ModeCharge, 1000)
	// After 2s (= τ) should be ~63% of steady-state
	// Steady-state with gain 0.9 = 900W
	for i := 0; i < 20; i++ { // 20 × 0.1s = 2s
		s.Tick(100 * time.Millisecond)
	}
	snap := s.Tick(0)
	expectedAt1Tau := 0.632 * 900
	if math.Abs(snap.ActualBatW-expectedAt1Tau) > 100 {
		t.Errorf("at τ=2s expected ~%.0fW, got %.0fW", expectedAt1Tau, snap.ActualBatW)
	}
}

func TestSoCClampedAt100(t *testing.T) {
	cfg := Default()
	cfg.SoC = 0.99
	cfg.ResponseTauS = 0.01
	cfg.CapacityWh = 100 // tiny, will saturate fast
	s := New(cfg)
	s.SetMode(ModeCharge, 5000)
	for i := 0; i < 120; i++ { // 2 minutes simulated
		s.Tick(time.Second)
	}
	snap := s.Tick(0)
	if snap.SoC > 1.0 {
		t.Errorf("SoC must cap at 1.0, got %f", snap.SoC)
	}
}

func TestSoCClampedAt0(t *testing.T) {
	cfg := Default()
	cfg.SoC = 0.01
	cfg.ResponseTauS = 0.01
	cfg.CapacityWh = 100
	s := New(cfg)
	s.SetMode(ModeDischarge, 5000)
	for i := 0; i < 120; i++ {
		s.Tick(time.Second)
	}
	snap := s.Tick(0)
	if snap.SoC < 0 {
		t.Errorf("SoC must floor at 0.0, got %f", snap.SoC)
	}
}

func TestSoCDeratingNearFull(t *testing.T) {
	cfg := Default()
	cfg.SoC = 0.98
	cfg.ResponseTauS = 0.01
	s := New(cfg)
	s.SetMode(ModeCharge, 5000)
	s.Tick(time.Second)
	s.Tick(time.Second)
	snap := s.Tick(time.Second)
	// At 98% SoC, max charge = 5000 * (1-0.98)/0.05 = 5000 * 0.4 = 2000W
	// Actual with gain should be ≤ ~2000 * 0.9 = 1800W
	if snap.ActualBatW > 2200 {
		t.Errorf("charge should derate near 98%% SoC, got %f", snap.ActualBatW)
	}
}

func TestGridBalance(t *testing.T) {
	cfg := Default()
	cfg.ResponseTauS = 0.01
	cfg.HouseJitterW = 0 // no noise for predictability
	cfg.HouseBaseW = 1000
	cfg.PVPeakW = 0 // will hit time-of-day curve; force via clock
	// Freeze clock to midnight so PV=0
	cfg.Clock = func() time.Time { return time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC) }
	s := New(cfg)
	s.SetMode(ModeAuto, 0)
	for i := 0; i < 5; i++ {
		s.Tick(time.Second)
	}
	snap := s.Tick(time.Second)
	// Midnight: PV=0, battery target=0 (actual→0), load=1000
	// Grid should equal load ≈ 1000W
	if math.Abs(snap.GridW-1000) > 200 {
		t.Errorf("expected grid ≈ load (1000W) at midnight auto-mode, got %f", snap.GridW)
	}
}

func TestPVTimeOfDayCurve(t *testing.T) {
	cfg := Default()
	cfg.PVPeakW = 0
	// Test multiple times of day
	cases := []struct {
		hour int
		minPV float64
		maxPV float64
	}{
		{3, 0, 0},          // night
		{13, 4000, 7000},   // peak
		{22, 0, 0},         // night again
	}
	for _, tc := range cases {
		cfg.Clock = func() time.Time {
			return time.Date(2026, 6, 1, tc.hour, 0, 0, 0, time.UTC)
		}
		s := New(cfg)
		snap := s.Tick(time.Second)
		if snap.PVW < tc.minPV-100 || snap.PVW > tc.maxPV+100 {
			t.Errorf("hour %d: PV %.0fW not in [%.0f, %.0f]", tc.hour, snap.PVW, tc.minPV, tc.maxPV)
		}
	}
}

func TestEnergyCountersMonotonic(t *testing.T) {
	s := New(Default())
	s.SetMode(ModeDischarge, 1000)
	snap1 := s.Tick(10 * time.Second)
	snap2 := s.Tick(10 * time.Second)
	if snap2.BatDischargeWh <= snap1.BatDischargeWh {
		t.Errorf("discharge Wh should grow: %f → %f", snap1.BatDischargeWh, snap2.BatDischargeWh)
	}
}

func TestAutoModeRelaxesToZero(t *testing.T) {
	cfg := Default()
	cfg.ResponseTauS = 0.1
	s := New(cfg)
	s.SetMode(ModeCharge, 2000)
	for i := 0; i < 10; i++ {
		s.Tick(time.Second)
	}
	snap := s.Tick(time.Second)
	if snap.ActualBatW < 1000 {
		t.Fatal("setup: should be charging strongly before auto")
	}
	s.SetMode(ModeAuto, 0)
	for i := 0; i < 20; i++ {
		s.Tick(time.Second)
	}
	snap = s.Tick(time.Second)
	if math.Abs(snap.ActualBatW) > 100 {
		t.Errorf("auto mode should relax actual to near zero, still at %f", snap.ActualBatW)
	}
}
