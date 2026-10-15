package core

import (
	"encoding/base64"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/metacubex/mihomo/config"
)

func TestNormalizedEnvelopeAndSubscription(t *testing.T) {
	for _, content := range []string{
		`{"schema":1,"proxy":{"type":"socks5","name":"Fixture","server":"192.0.2.2","port":1080,"udp":true}}`,
		base64.StdEncoding.EncodeToString([]byte("socks5://192.0.2.2:1080#Fixture")),
	} {
		nodes, err := Import(content, "")
		if err != nil || len(nodes) != 1 {
			t.Fatalf("valid portable input rejected: %v", err)
		}
	}
}

func TestMyceliaBundleImport(t *testing.T) {
	if os.Getenv("MUKHOMOR_TEST_MYCELIA") != "1" {
		t.Skip("requires reviewed Mycelia source bundle")
	}
	seed := strings.Repeat("ab", 32)
	nodes, err := Import("mycelium://"+seed+"@192.0.2.2:443?sni=fixture.invalid&udp=true#Fixture", "")
	if err != nil || len(nodes) != 1 {
		t.Fatalf("Mycelia import failed: %v", err)
	}
	if nodes[0]["seed"] != seed || nodes[0]["sni"] != "fixture.invalid" {
		t.Fatal("Mycelia identity changed")
	}
	if _, err = Import("mycelium://"+seed+"@192.0.2.2:443?sni=one&sni=two", ""); err == nil {
		t.Fatal("duplicate identity accepted")
	}
}

type fakeEngine struct {
	running bool
	fail    bool
	started chan struct{}
	resume  chan struct{}
}

func (f *fakeEngine) Start(Object) error {
	if f.started != nil {
		close(f.started)
		<-f.resume
	}
	if f.fail {
		return errors.New("Synthetic TUN failure")
	}
	f.running = true
	return nil
}
func (f *fakeEngine) Stop() error   { f.running = false; return nil }
func (f *fakeEngine) Running() bool { return f.running }
func request(t *testing.T, c *Controller, value Object) Object {
	t.Helper()
	b, _ := json.Marshal(value)
	var r Object
	if err := json.Unmarshal(c.Request(b, -1), &r); err != nil {
		t.Fatal(err)
	}
	return r
}
func fixture(t *testing.T, f Engine) (*Controller, string) {
	t.Helper()
	root := filepath.Join(t.TempDir(), "private")
	c, e := New(root, "linux", f)
	if e != nil {
		t.Fatal(e)
	}
	t.Cleanup(c.Close)
	return c, root
}

const socksNode = `{"name":"Test server","type":"socks5","server":"192.0.2.1","port":1080,"udp":true}`

func TestImportAtomicPrivateLibraryAndReceipts(t *testing.T) {
	c, root := fixture(t, &fakeEngine{})
	r := request(t, c, Object{"action": "Import", "content": socksNode, "request_id": "one"})
	if r["ok"] != true {
		t.Fatal(r["error"])
	}
	r = request(t, c, Object{"action": "Import", "content": socksNode, "request_id": "one"})
	if len(array(obj(r["data"])["profiles"])) != 1 {
		t.Fatal("Duplicate receipt imported twice")
	}
	info, e := os.Stat(filepath.Join(root, "library.json"))
	if e != nil || info.Mode().Perm() != 0600 {
		t.Fatal("Credentials are not private")
	}
	r = request(t, c, Object{"action": "Import", "content": socksNode + "\nnot-a-server", "request_id": "two"})
	if r["ok"] == true || len(array(obj(r["data"])["profiles"])) != 1 {
		t.Fatal("Malformed batch partially committed")
	}
	if _, err := OpenStore(root); err == nil {
		t.Fatal("Second controller obtained the library lock")
	}
	serialized, _ := json.Marshal(obj(r["data"])["profiles"])
	if strings.Contains(string(serialized), "192.0.2.1") {
		t.Fatal("Snapshot exposed endpoint")
	}
}

func TestFailureDoesNotRecordConnectionHistory(t *testing.T) {
	f := &fakeEngine{fail: true}
	c, _ := fixture(t, f)
	request(t, c, Object{"action": "Import", "content": socksNode})
	r := request(t, c, Object{"action": "Connect"})
	if r["ok"] == true || obj(r["data"])["last_connected"] != "" || f.running {
		t.Fatal("Failed connection recorded as successful", r["error"])
	}
	f.fail = false
	r = request(t, c, Object{"action": "Connect"})
	if r["ok"] != true || obj(r["data"])["last_connected"] == "" {
		t.Fatal(r["error"])
	}
	r = request(t, c, Object{"action": "Remove", "id": obj(r["data"])["selected"]})
	if r["ok"] == true {
		t.Fatal("Removed active profile")
	}
	r = request(t, c, Object{"action": "Disconnect"})
	if r["ok"] != true || obj(r["data"])["running"] != false {
		t.Fatal("Disconnect failed")
	}
}

func TestCancellationCannotCommitLateStart(t *testing.T) {
	f := &fakeEngine{started: make(chan struct{}), resume: make(chan struct{})}
	c, _ := fixture(t, f)
	request(t, c, Object{"action": "Import", "content": socksNode})
	finished := make(chan []byte, 1)
	go func() { finished <- c.Request([]byte(`{"action":"Connect"}`), -1) }()
	<-f.started
	c.cancel.Add(1)
	close(f.resume)
	<-finished
	r := request(t, c, Object{"action": "Status"})
	if f.running || obj(r["data"])["last_connected"] != "" {
		t.Fatal("Cancelled start kept the tunnel or history")
	}
}

func TestImportRejectsFileAccessProvidersAndAliases(t *testing.T) {
	bad := []string{
		`{"name":"n","type":"socks5","server":"a","port":1080,"dialer-proxy":"other"}`,
		`{"proxies":[],"dns":{"nameserver":["attacker"]}}`,
		`{"type":"ss","TYPE":"socks5","server":"a","port":1}`,
		"a: &a [*a]", "proxies: [1]",
		`{"name":"n","type":"http","server":"a","port":80,"certificate":"/etc/passwd"}`,
		`{"name":"n","type":"ssh","server":"a","port":22,"username":"n","private-key":"/etc/passwd"}`,
		strings.Repeat("x", MaxImport+1),
	}
	for _, text := range bad {
		if _, err := Import(text, ""); err == nil {
			t.Fatalf("Accepted unsafe input %.80s", text)
		}
	}
}

func TestLibrarySymlinksAndWorldReadableDirectoriesFail(t *testing.T) {
	root := t.TempDir()
	if err := os.Chmod(root, 0755); err != nil {
		t.Fatal(err)
	}
	if s, err := OpenStore(root); err == nil {
		s.Close()
		t.Fatal("World-readable root accepted")
	}
	if err := os.Chmod(root, 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink("outside", filepath.Join(root, "library.json")); err != nil {
		t.Fatal(err)
	}
	s, err := OpenStore(root)
	if err != nil {
		t.Fatal(err)
	}
	defer s.Close()
	if _, err = s.Read(); err == nil {
		t.Fatal("Symlink library accepted")
	}
}

func TestRealCoreParsesBothPlatformConfigurations(t *testing.T) {
	nodes, err := Import(socksNode, "")
	if err != nil {
		t.Fatal(err)
	}
	for _, platform := range []string{"linux", "android"} {
		for _, mode := range []string{"split", "full"} {
			s := defaultSettings()
			obj(s["routing"])["mode"] = mode
			cfg, err := makeConfig(nodes[0], s, platform, 42, nil)
			if err != nil {
				t.Fatal(platform, mode, err)
			}
			raw, _ := json.Marshal(cfg)
			parsed, err := config.Parse(raw)
			if err != nil {
				t.Fatal(platform, mode, err)
			}
			if platform == "android" && (parsed.General.Tun.Inet4Address[0].String() != "198.18.0.1/30" || cfg["tun"].(Object)["stack"] != "gvisor") {
				t.Fatal("Android VpnService address/stack contract changed")
			}
			if cfg["external-controller"] != "" || cfg["mixed-port"] != 0 {
				t.Fatal("Exposed unauthenticated control or proxy port")
			}
			if mode == "full" {
				for _, rule := range cfg["rules"].([]string) {
					if strings.HasPrefix(rule, "PROCESS-") || strings.HasPrefix(rule, "DOMAIN") {
						t.Fatal("Full mode retained direct exceptions")
					}
				}
			}
		}
	}
}

func TestAndroidPackageRulesAndValidation(t *testing.T) {
	s := defaultSettings()
	obj(s["direct"])["process_names"] = []any{"org.example.game", "game.exe", "bad,rule"}
	issues, err := validateSettings(s)
	if err != nil || len(issues) == 0 {
		t.Fatal("Invalid rule was not reported")
	}
	packages := androidPackages(s)
	if len(packages) != 1 || packages[0] != "org.example.game" {
		t.Fatal(packages)
	}
	obj(s["routing"])["mode"] = "full"
	if len(androidPackages(s)) != 0 {
		t.Fatal("Full mode bypasses apps")
	}
}
