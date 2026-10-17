package core

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"reflect"
	"strings"
	"testing"

	"github.com/metacubex/mihomo/config"
)

func TestTorFirstRunPresetPreservesSavedChoice(t *testing.T) {
	c := &Controller{data: Object{}}
	if options := c.torOptions(); options["mode"] != "snowflake" || options["bridges"] != "" {
		t.Fatal("first Tor connection requires manual bridge configuration")
	}
	c.data["tor_options"] = Object{"mode": "direct", "bridges": ""}
	if c.torOptions()["mode"] != "direct" {
		t.Fatal("default Tor preset overwrote an explicit saved choice")
	}
}

func TestTorPolicyHasNoDirectOrUDPPath(t *testing.T) {
	settings := defaultSettings()
	obj(settings["direct"])["domains"] = []any{"example.com"}
	before, _ := json.Marshal(settings)
	for _, platform := range []string{"linux", "android"} {
		cfg, err := makeTorConfig(Object{"type": "socks5", "server": "127.0.0.1", "port": 12345, "username": strings.Repeat("a", 64), "password": strings.Repeat("b", 64), "udp": false}, settings, platform, 123)
		if err != nil {
			t.Fatal(err)
		}
		if !reflect.DeepEqual(cfg["rules"], []string{"NETWORK,udp,REJECT", "MATCH,VPN"}) {
			t.Fatal("Tor policy permits a bypass")
		}
		for _, key := range []string{"nameserver", "default-nameserver", "proxy-server-nameserver", "direct-nameserver"} {
			for _, resolver := range obj(cfg["dns"])[key].([]string) {
				if !strings.HasSuffix(resolver, "#VPN") || !strings.HasPrefix(resolver, "https://") {
					t.Fatal("DNS can bypass Tor")
				}
			}
		}
		// Parse with the actual bundled engine, without opening a TUN.
		obj(cfg["tun"])["enable"] = false
		delete(obj(cfg["tun"]), "file-descriptor")
		b, _ := json.Marshal(cfg)
		if _, err = config.Parse(b); err != nil {
			t.Fatalf("%s Tor configuration rejected: %v", platform, err)
		}
	}
	after, _ := json.Marshal(settings)
	if string(before) != string(after) {
		t.Fatal("Tor changed saved split settings")
	}
}

func TestFailedTorTransportRetainsTunnelForExplicitDisconnect(t *testing.T) {
	fake := &fakeEngine{running: true}
	dir := t.TempDir()
	_ = os.Chmod(dir, 0700)
	c, err := New(dir, "linux", fake)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	c.network = "tor"
	c.tor = &torSession{} // dead transport; the TUN must remain active
	s := c.snapshot()
	if s["running"] != false || s["phase"] != "recovery" || s["can_disconnect"] != true || !fake.running {
		t.Fatal("transport failure dropped or misreported the blocking TUN")
	}
	c.tor = nil
	if response := request(t, c, Object{"action": "Disconnect"}); response["ok"] != true || fake.running {
		t.Fatal("explicit disconnect failed")
	}
}

func TestTorModeRejectsPolicySwitches(t *testing.T) {
	dir := t.TempDir()
	_ = os.Chmod(dir, 0700)
	c, err := New(dir, "linux", &fakeEngine{running: true})
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	c.network = "tor"
	for _, r := range []Object{{"action": "Connect", "network": Object{"invalid": "tor"}}, {"action": "Select", "id": "unused"}, {"action": "ApplySettings", "settings": defaultSettings()}, {"action": "Autoconnect", "enabled": true}} {
		if response := request(t, c, r); response["ok"] != false {
			t.Fatal("Tor allowed a live policy switch")
		}
	}
}

func TestConnectionFailureBeforeFirstPollInvalidatesRevision(t *testing.T) {
	dir := t.TempDir()
	_ = os.Chmod(dir, 0700)
	engine := &fakeEngine{}
	c, err := New(dir, "linux", engine)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	id := strings.Repeat("a", 32)
	c.data["selected"] = id
	c.data["profiles"] = []any{Object{"id": id, "name": "fixture", "node": Object{"type": "socks5", "server": "192.0.2.1", "port": 1080}}}
	connected := request(t, c, Object{"action": "Connect"})
	if connected["ok"] != true {
		t.Fatal(connected["error"])
	}
	engine.running = false
	status := request(t, c, Object{"action": "Status", "if_revision": connected["revision"]})
	if status["not_modified"] == true || obj(status["data"])["running"] != false {
		t.Fatal("failure before the first poll was hidden")
	}
}

func TestTorStorageFailureLeavesDisconnectAvailable(t *testing.T) {
	dir := t.TempDir()
	_ = os.Chmod(dir, 0700)
	c, err := New(dir, "android", &fakeEngine{})
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	_ = c.store.dir.Close() // Simulate unavailable private storage before bootstrap.
	response := request(t, c, Object{"action": "Connect", "network": "tor"})
	data := obj(response["data"])
	if response["ok"] != false || data["can_disconnect"] != true || data["phase"] != "recovery" {
		t.Fatal("held Android TUN has no explicit disconnect after a storage error")
	}
}

func TestConnectCannotSilentlyChangeNetworkOrAcceptDeadTor(t *testing.T) {
	for _, tc := range []struct {
		active, requested string
		alive, wantOK     bool
	}{
		{"", "tor", false, false},
		{"vps", "tor", false, false},
		{"tor", "vps", true, false},
		{"tor", "", true, false},
		{"tor", "tor", false, false},
		{"tor", "tor", true, true},
		{"vps", "vps", false, true},
	} {
		t.Run(tc.active+"_to_"+tc.requested+"_alive_"+fmt.Sprint(tc.alive), func(t *testing.T) {
			dir := t.TempDir()
			_ = os.Chmod(dir, 0700)
			engine := &fakeEngine{running: true}
			c, err := New(dir, "android", engine)
			if err != nil {
				t.Fatal(err)
			}
			defer func() { c.tor = nil; c.Close() }()
			c.network = tc.active
			if tc.active == "tor" {
				c.tor = &torSession{}
				c.tor.alive.Store(tc.alive)
			}
			reply := request(t, c, Object{"action": "Connect", "network": tc.requested})
			if (reply["ok"] == true) != tc.wantOK {
				t.Fatalf("unexpected connect result: %v", reply)
			}
			if !engine.running || c.network != tc.active {
				t.Fatal("rejected switch disturbed the active tunnel")
			}
		})
	}
}

type retainedTestGuard struct {
	active, refuse bool
	removals       int
}

func (g *retainedTestGuard) present() (bool, error) { return g.active, nil }
func (g *retainedTestGuard) arm() error             { g.active = true; return nil }
func (g *retainedTestGuard) disarm() error {
	g.removals++
	if g.refuse {
		return errors.New("synthetic firewall removal failure")
	}
	g.active = false
	return nil
}

func TestTorGuardFailureRequiresExplicitRecovery(t *testing.T) {
	dir := t.TempDir()
	_ = os.Chmod(dir, 0700)
	c, err := New(dir, "linux", &fakeEngine{running: true})
	if err != nil {
		t.Fatal(err)
	}
	g := &retainedTestGuard{active: true, refuse: true}
	c.guard = g
	c.guardArmed = true
	c.network = "tor"
	if r := request(t, c, Object{"action": "Disconnect"}); r["ok"] != false || obj(r["data"])["can_disconnect"] != true {
		t.Fatal("failed removal lost recovery")
	}
	if r := request(t, c, Object{"action": "Connect", "network": "vps"}); r["ok"] != false {
		t.Fatal("retained guard allowed network switch")
	}
	g.refuse = false
	if r := request(t, c, Object{"action": "Disconnect"}); r["ok"] != true || g.active {
		t.Fatal("explicit recovery failed")
	}
	removals := g.removals
	c.Close()
	if g.removals != removals {
		t.Fatal("implicit controller shutdown disarmed the firewall")
	}
}
