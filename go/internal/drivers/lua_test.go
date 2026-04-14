package drivers

import (
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/frahlg/forty-two-watts/go/internal/telemetry"
)

// Minimal driver that emits meter + battery via host.emit and bumps the
// poll counter. Exercises the full host API.
const testDriverSrc = `
host.set_make("TestMaker")
host.set_sn("SN-42")
tick = 0
function driver_init(config)
    host.log("info", "init called")
    assert(config ~= nil, "config should be passed")
    assert(config.foo == "bar", "config.foo should be 'bar'")
end
function driver_poll()
    tick = tick + 1
    host.emit("meter", { w = tick * 100 })
    host.emit("battery", { w = -500, soc = 0.87 })
    return 1000
end
function driver_command(action, w, cmd)
    host.log("info", "cmd: " .. tostring(action) .. " w=" .. tostring(w))
    assert(cmd.action == action, "cmd.action matches")
end
`

func TestLuaDriverLifecycle(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "test.lua")
	if err := os.WriteFile(path, []byte(testDriverSrc), 0644); err != nil {
		t.Fatal(err)
	}
	tel := telemetry.NewStore()
	env := NewHostEnv("test", tel)

	d, err := NewLuaDriver(path, env)
	if err != nil {
		t.Fatalf("load: %v", err)
	}
	defer d.Cleanup()

	// Init with config.
	if err := d.Init(context.Background(), map[string]any{"foo": "bar"}); err != nil {
		t.Fatalf("init: %v", err)
	}

	// SN + make captured.
	mk, sn := env.Identity()
	if mk != "TestMaker" || sn != "SN-42" {
		t.Errorf("identity: got (%q, %q)", mk, sn)
	}

	// Poll three times, check telemetry.
	for i := 0; i < 3; i++ {
		next, err := d.Poll(context.Background())
		if err != nil {
			t.Fatalf("poll: %v", err)
		}
		if next != 1000*time.Millisecond {
			t.Errorf("next poll: %v", next)
		}
	}
	meter := tel.Get("test", telemetry.DerMeter)
	if meter == nil || meter.RawW != 300 {
		t.Errorf("meter: %+v", meter)
	}
	bat := tel.Get("test", telemetry.DerBattery)
	if bat == nil || bat.SoC == nil || *bat.SoC != 0.87 {
		t.Errorf("battery: %+v (soc=%v)", bat, bat.SoC)
	}

	// Command.
	err = d.Command(context.Background(), []byte(`{"action":"set","w":-1500}`))
	if err != nil {
		t.Fatalf("command: %v", err)
	}
}

func TestLuaDriverMissingFile(t *testing.T) {
	env := NewHostEnv("test", telemetry.NewStore())
	_, err := NewLuaDriver("/nonexistent/path.lua", env)
	if err == nil {
		t.Error("expected error for missing file")
	}
}

func TestLuaDriverSyntaxError(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "bad.lua")
	os.WriteFile(path, []byte("function (x"), 0644)
	env := NewHostEnv("bad", telemetry.NewStore())
	_, err := NewLuaDriver(path, env)
	if err == nil {
		t.Error("expected parse error")
	}
}
