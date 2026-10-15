package core

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"strings"
	"sync"
	"sync/atomic"

	assets "github.com/0nionow1/Mukhomor"
	"github.com/metacubex/mihomo/config"
	C "github.com/metacubex/mihomo/constant"
	"github.com/metacubex/mihomo/hub/executor"
	"github.com/metacubex/mihomo/listener"
	LC "github.com/metacubex/mihomo/listener/config"
	"github.com/metacubex/mihomo/log"
	"github.com/metacubex/mihomo/tunnel"
	"github.com/metacubex/mihomo/tunnel/statistic"
	logrus "github.com/sirupsen/logrus"
)

type Engine interface {
	Start(Object) error
	Stop() error
	Running() bool
}
type Mihomo struct{ running bool }

func (m *Mihomo) Start(value Object) error {
	b, _ := json.Marshal(value)
	cfg, err := config.Parse(b)
	if err != nil {
		return errors.New("VPN configuration is not valid for the bundled core")
	}
	// Capture only the TUN initialization error. General connection logs can
	// contain private server names and must not enter controller responses.
	sub := log.Subscribe()
	done := make(chan struct{})
	var tunError string
	go func() {
		defer close(done)
		for event := range sub {
			if strings.HasPrefix(event.Payload, "Start TUN listening error:") {
				tunError = event.Payload
			}
		}
	}()
	executor.ApplyConfig(cfg, true)
	log.UnSubscribe(sub)
	<-done
	if !listener.GetTunConf().Enable {
		executor.Shutdown()
		if tunError != "" {
			return errors.New(tunError)
		}
		return errors.New("Cannot start TUN. Check VPN permission or Linux installation privileges")
	}
	m.running = true
	return nil
}
func (m *Mihomo) Stop() error {
	if m.running {
		// Shutdown closes the listener but retains LastTunConf upstream.
		// Explicitly disable it so an identical reconnect (including a reused
		// Android FD number) creates a new reader instead of reloading nil.
		listener.ReCreateTun(LC.Tun{}, tunnel.Tunnel)
		executor.Shutdown()
		statistic.DefaultManager.Range(func(t statistic.Tracker) bool { _ = t.Close(); return true })
	}
	m.running = false
	return nil
}
func (m *Mihomo) Running() bool { return m.running && listener.GetTunConf().Enable }

type Controller struct {
	mu              sync.Mutex
	store           *Store
	data            Object
	engine          Engine
	platform        string
	revision        uint64
	epoch           string
	last            Object
	cancel          atomic.Uint64
	issues          []any
	observedRunning bool
	diagnostics     Object
}

func New(path, platform string, engine Engine) (*Controller, error) {
	logrus.SetOutput(io.Discard)
	store, err := OpenStore(path)
	if err != nil {
		return nil, err
	}
	data, err := store.Read()
	if err != nil {
		store.Close()
		return nil, err
	}
	if data == nil {
		data = Object{"schema": 1, "selected": "", "last_connected": "", "autoconnect": false, "profiles": []any{}, "settings": defaultSettings()}
	}
	if data["schema"] != float64(1) && data["schema"] != 1 {
		store.Close()
		return nil, errors.New("Unsupported profile library schema")
	}
	seen := map[string]bool{}
	for _, v := range array(data["profiles"]) {
		p := obj(v)
		id := str(p["id"])
		if !validID.MatchString(id) || seen[id] || !validName(str(p["name"])) {
			store.Close()
			return nil, errors.New("Invalid profile library")
		}
		seen[id] = true
	}
	if len(seen) > 512 {
		store.Close()
		return nil, errors.New("Too many saved profiles")
	}
	for _, key := range []string{"selected", "last_connected"} {
		if s := str(data[key]); s != "" && !seen[s] {
			store.Close()
			return nil, errors.New("Profile selection is damaged")
		}
	}
	if _, err = validateSettings(obj(data["settings"])); err != nil {
		store.Close()
		return nil, err
	}
	if engine == nil {
		engine = &Mihomo{}
		C.SetHomeDir(path)
	}
	return &Controller{store: store, data: data, engine: engine, platform: platform, epoch: randomID(), issues: []any{}}, nil
}
func (c *Controller) Close() {
	c.cancel.Add(1)
	c.mu.Lock()
	defer c.mu.Unlock()
	_ = c.engine.Stop()
	c.store.Close()
}
func (c *Controller) snapshot() Object {
	profiles := []any{}
	for _, v := range array(c.data["profiles"]) {
		p := obj(v)
		profiles = append(profiles, Object{"id": p["id"], "name": p["name"], "protocol": obj(p["node"])["type"]})
	}
	running := c.engine.Running()
	phase := "idle"
	if running {
		phase = "connected"
	}
	return Object{"profiles": profiles, "settings": c.data["settings"], "selected": c.data["selected"], "last_connected": c.data["last_connected"], "autoconnect": c.data["autoconnect"], "running": running, "phase": phase, "service_ready": true, "initializing": false, "busy": false, "can_disconnect": running, "traffic_guard": Object{"protected": false, "configured": false}, "last_operation": c.last, "rule_issues": c.issues, "rule_issues_total": len(c.issues), "version": assets.Version, "platform": c.platform, "diagnostics": c.diagnostics}
}
func (c *Controller) Request(raw []byte, fd int) []byte {
	fail := func(message string) []byte { b, _ := json.Marshal(Object{"ok": false, "error": message}); return b }
	if len(raw) > MaxFrame {
		return fail("Request exceeds size limit")
	}
	var request Object
	if json.Unmarshal(raw, &request) != nil {
		return fail("Invalid request JSON")
	}
	action := str(request["action"])
	if action == "Disconnect" || action == "Exit" {
		c.cancel.Add(1)
	}
	generation := c.cancel.Load()
	c.mu.Lock()
	defer c.mu.Unlock()
	if current := c.engine.Running(); current != c.observedRunning {
		c.observedRunning = current
		c.revision++
	}
	if action != "Status" && str(request["request_id"]) != "" && str(c.last["request_id"]) == str(request["request_id"]) {
		b, _ := json.Marshal(Object{"ok": c.last["ok"], "error": c.last["error"], "request_id": request["request_id"], "revision": fmt.Sprint(c.epoch, "-", c.revision), "data": c.snapshot()})
		return b
	}
	if action == "Status" {
		rev := fmt.Sprint(c.epoch, "-", c.revision)
		if request["if_revision"] == rev {
			b, _ := json.Marshal(Object{"ok": true, "not_modified": true, "revision": rev})
			return b
		}
	}
	err := c.apply(request, fd, generation)
	message := ""
	if err != nil {
		message = err.Error()
	}
	if action != "Status" {
		c.revision++
		c.last = Object{"request_id": request["request_id"], "action": action, "ok": err == nil, "error": message}
	}
	b, e := json.Marshal(Object{"ok": err == nil, "error": message, "request_id": request["request_id"], "revision": fmt.Sprint(c.epoch, "-", c.revision), "data": c.snapshot()})
	if e != nil {
		return fail("Cannot encode controller state")
	}
	return b
}
func (c *Controller) apply(r Object, fd int, generation uint64) error {
	action := str(r["action"])
	if action == "Status" {
		return nil
	}
	if action == "Disconnect" || action == "Exit" {
		return c.engine.Stop()
	}
	if action == "Connect" {
		if c.engine.Running() {
			return nil
		}
		var node Object
		target := c.data["selected"]
		if r["startup"] == true {
			target = c.data["last_connected"]
		}
		for _, v := range array(c.data["profiles"]) {
			p := obj(v)
			if p["id"] == target {
				node = obj(p["node"])
			}
		}
		if node == nil {
			return errors.New("Select a server first")
		}
		cfg, err := makeConfig(node, obj(c.data["settings"]), c.platform, fd, obj(c.data["rule_lists"]))
		if err != nil {
			return err
		}
		if c.cancel.Load() != generation {
			return errors.New("Connection cancelled")
		}
		if err = c.engine.Start(cfg); err != nil {
			return err
		}
		if c.cancel.Load() != generation {
			_ = c.engine.Stop()
			return errors.New("Connection cancelled")
		}
		next := obj(clone(c.data))
		next["selected"] = target
		next["last_connected"] = next["selected"]
		if err = c.store.Write(next); err != nil {
			_ = c.engine.Stop()
			return errors.New("Cannot save successful connection")
		}
		c.data = next
		return nil
	}
	if action == "Diagnose" {
		issues, err := validateSettings(obj(c.data["settings"]))
		if err != nil {
			return err
		}
		c.diagnostics = Object{"version": assets.Version, "platform": c.platform, "tunnel_active": c.engine.Running(), "profiles": len(array(c.data["profiles"])), "invalid_rules": len(issues), "private_storage": "0700 / 0600", "rules_updated_at": c.data["rules_updated_at"]}
		return nil
	}
	if action == "PrepareAndroid" {
		return nil
	}
	if action == "Update" {
		return c.updateRules()
	}
	if action == "ValidateSettings" {
		issues, err := validateSettings(obj(r["settings"]))
		c.issues = issues
		return err
	}
	next := obj(clone(c.data))
	profiles := array(next["profiles"])
	switch action {
	case "Import":
		nodes, err := Import(str(r["content"]), str(r["name"]))
		if err != nil {
			return err
		}
		if len(profiles)+len(nodes) > 512 {
			return errors.New("Profile library limit is 512 servers")
		}
		for _, node := range nodes {
			id := randomID()
			profiles = append(profiles, Object{"id": id, "name": node["name"], "node": node})
			if str(next["selected"]) == "" {
				next["selected"] = id
			}
		}
		next["profiles"] = profiles
	case "Select", "Rename", "Remove":
		id := str(r["id"])
		index := -1
		for i, v := range profiles {
			if obj(v)["id"] == id {
				index = i
			}
		}
		if index < 0 {
			return errors.New("Server not found")
		}
		if (action == "Select" || action == "Remove") && c.engine.Running() {
			return errors.New("Disconnect before changing the active profile")
		}
		if action == "Select" {
			next["selected"] = id
		}
		if action == "Rename" {
			name := strings.TrimSpace(str(r["name"]))
			if !validName(name) {
				return errors.New("Invalid server name")
			}
			obj(profiles[index])["name"] = name
			obj(obj(profiles[index])["node"])["name"] = name
		}
		if action == "Remove" {
			next["profiles"] = append(profiles[:index], profiles[index+1:]...)
			if next["selected"] == id {
				next["selected"] = ""
				if len(array(next["profiles"])) > 0 {
					next["selected"] = obj(array(next["profiles"])[0])["id"]
				}
			}
			if next["last_connected"] == id {
				next["last_connected"] = ""
			}
		}
	case "Autoconnect":
		if _, ok := r["enabled"].(bool); !ok {
			return errors.New("Invalid autoconnect preference")
		}
		next["autoconnect"] = r["enabled"]
	case "ApplySettings":
		if c.engine.Running() {
			return errors.New("Disconnect before changing routing settings")
		}
		issues, err := validateSettings(obj(r["settings"]))
		c.issues = issues
		if err != nil {
			return err
		}
		if len(issues) > 0 {
			return errors.New("Fix invalid rules before saving")
		}
		next["settings"] = r["settings"]
	default:
		return errors.New("Unknown controller operation")
	}
	if err := c.store.Write(next); err != nil {
		return errors.New("Cannot save private application data")
	}
	c.data = next
	return nil
}

func (c *Controller) AndroidPackages() []string {
	c.mu.Lock()
	defer c.mu.Unlock()
	return androidPackages(obj(c.data["settings"]))
}
