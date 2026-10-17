package core

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync/atomic"
	"time"
)

func isP2P(network string) bool { return network == "tor" || network == "i2p" }

func (c *Controller) torOptions() Object {
	if options, ok := c.data["tor_options"].(Object); ok {
		return options
	}
	return Object{"mode": "snowflake", "bridges": ""}
}
func (c *Controller) torPath() (string, error) {
	path := c.torHelper
	if path == "" {
		executable, err := os.Executable()
		if err != nil {
			return "", errors.New("Cannot locate the bundled Tor transport")
		}
		path = filepath.Join(filepath.Dir(executable), "mukhomor-tor")
	}
	info, err := os.Lstat(path)
	if err != nil || !info.Mode().IsRegular() {
		return "", errors.New("Bundled Tor transport is missing; reinstall Mukhomor")
	}
	return path, nil
}
func (c *Controller) validateTorOptions(value any) (Object, error) {
	options, ok := value.(Object)
	if !ok || len(options) != 2 {
		return nil, errors.New("Invalid Tor bridge configuration")
	}
	mode, mok := options["mode"].(string)
	bridges, bok := options["bridges"].(string)
	if !mok || !bok || len(bridges) > 8192 || (mode != "direct" && mode != "obfs4" && mode != "webtunnel" && mode != "snowflake" && mode != "snowflake-amp") {
		return nil, errors.New("Invalid Tor bridge configuration")
	}
	path, err := c.torPath()
	if err != nil {
		return nil, err
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, path, "--validate-options")
	frame, _ := json.Marshal(options)
	cmd.Stdin = bytes.NewReader(frame)
	out, err := cmd.Output()
	if err != nil || len(out) > 32768 {
		return nil, errors.New("Invalid Tor bridge configuration")
	}
	var reply struct {
		Options Object `json:"options"`
	}
	if json.Unmarshal(out, &reply) != nil || reply.Options == nil {
		return nil, errors.New("Invalid Tor bridge configuration")
	}
	return reply.Options, nil
}

// Closing the supervisor pipe terminates Arti, even during bootstrap.
type torSession struct {
	cmd   *exec.Cmd
	input io.WriteCloser
	done  chan struct{}
	alive atomic.Bool
}

// The Linux guard lives in the kernel and outlives the controller. Android
// retains its VpnService TUN; tests inject engines and never modify host policy.
type torGuard interface {
	present() (bool, error)
	arm() error
	disarm() error
}
type noTorGuard struct{}

func (noTorGuard) present() (bool, error) { return false, nil }
func (noTorGuard) arm() error             { return nil }
func (noTorGuard) disarm() error          { return nil }

func (t *torSession) stop() {
	if t == nil {
		return
	}
	_ = t.input.Close()
	select {
	case <-t.done:
	case <-time.After(time.Second):
		_ = t.cmd.Process.Kill()
		<-t.done
	}
}
func torInterface(platform string) (string, error) {
	if platform == "android" {
		return "", nil
	}
	f, err := os.Open("/proc/net/route")
	if err != nil {
		return "", errors.New("Cannot find a physical interface for Tor")
	}
	defer f.Close()
	scan := bufio.NewScanner(f)
	for scan.Scan() {
		fields := strings.Fields(scan.Text())
		if len(fields) >= 4 && fields[1] == "00000000" && fields[2] != "00000000" && fields[0] != "mukhomor0" && !strings.ContainsAny(fields[0], "/\\:\x00") {
			return fields[0], nil
		}
	}
	return "", errors.New("No physical default route for Tor; disconnect other VPNs")
}
func (c *Controller) startTor(generation uint64) (*torSession, Object, error) {
	path, err := c.torPath()
	if err != nil {
		return nil, nil, err
	}
	if c.network == "i2p" {
		name := "mukhomor-pt"
		if c.platform == "android" {
			name = "libmukhomor_pt.so"
		}
		path = filepath.Join(filepath.Dir(path), name)
	}
	iface, err := torInterface(c.platform)
	if err != nil {
		return nil, nil, err
	}
	user, password := randomID()+randomID(), randomID()+randomID()
	base := filepath.Join(c.store.path, c.network)
	if err = os.MkdirAll(base, 0700); err != nil {
		return nil, nil, errors.New("Cannot create private Tor state")
	}
	init := Object{"state_dir": filepath.Join(base, "state"), "cache_dir": filepath.Join(base, "cache"), "username": user, "password": password, "interface": iface, "tor_options": c.torOptions()}
	if c.network == "i2p" {
		init = Object{"state_dir": filepath.Join(base, "state"), "interface": iface, "token": user, "password": password, "mode": "i2p", "bridges": []string{}}
	}
	cmd := exec.Command(path)
	input, err := cmd.StdinPipe()
	if err != nil {
		return nil, nil, errors.New("Cannot create Tor transport pipe")
	}
	output, err := cmd.StdoutPipe()
	if err != nil {
		_ = input.Close()
		return nil, nil, errors.New("Cannot create Tor transport pipe")
	}
	cmd.Stderr = io.Discard
	if err = cmd.Start(); err != nil {
		_ = input.Close()
		return nil, nil, errors.New("Cannot start bundled Tor transport")
	}
	t := &torSession{cmd: cmd, input: input, done: make(chan struct{})}
	t.alive.Store(true)
	go func() { _ = cmd.Wait(); t.alive.Store(false); close(t.done) }()
	b, _ := json.Marshal(init)
	if _, err = input.Write(append(b, '\n')); err != nil {
		t.stop()
		return nil, nil, errors.New("Tor transport initialization failed")
	}
	ready := make(chan Object, 1)
	go func() {
		var value Object
		line, e := bufio.NewReader(io.LimitReader(output, 4097)).ReadBytes('\n')
		if e == nil && len(line) <= 4096 {
			_ = json.Unmarshal(line, &value)
		}
		ready <- value
	}()
	budget := 300 * time.Second
	if c.network == "tor" && c.torOptions()["mode"] == "snowflake-amp" {
		budget = 420 * time.Second
	}
	if c.network == "i2p" {
		budget = 480 * time.Second
	}
	timer := time.NewTimer(budget)
	defer timer.Stop()
	tick := time.NewTicker(50 * time.Millisecond)
	defer tick.Stop()
	for {
		select {
		case value := <-ready:
			port, ok := value["port"].(float64)
			if value["ready"] != true || !ok || port < 1 || port > 65535 || port != float64(int(port)) || !t.alive.Load() {
				t.stop()
				return nil, nil, errors.New("P2P bootstrap failed; the selected network may be unavailable or blocked")
			}
			return t, Object{"type": "socks5", "name": "P2P", "server": "127.0.0.1", "port": int(port), "username": user, "password": password, "udp": false}, nil
		case <-t.done:
			t.stop()
			return nil, nil, errors.New("P2P transport stopped during bootstrap")
		case <-timer.C:
			t.stop()
			return nil, nil, errors.New("P2P bootstrap timed out; try another connector or Tor bridge")
		case <-tick.C:
			if _, markerErr := os.Stat(filepath.Join(c.store.path, "tor-cancel")); c.cancel.Load() != generation || markerErr == nil {
				t.stop()
				return nil, nil, errors.New("Connection cancelled")
			}
		}
	}
}
func makeTorConfig(node, settings Object, platform string, fd int) (Object, error) {
	s := obj(clone(settings))
	s["routing"] = Object{"mode": "full", "allow_lan": false}
	cfg, err := makeConfig(node, s, platform, fd, nil)
	if err != nil {
		return nil, err
	}
	cfg["rules"] = []string{"NETWORK,udp,REJECT", "MATCH,VPN"}
	dns := obj(cfg["dns"])
	resolvers := []string{"https://1.1.1.1/dns-query#VPN", "https://8.8.8.8/dns-query#VPN"}
	for _, key := range []string{"nameserver", "default-nameserver", "proxy-server-nameserver", "direct-nameserver"} {
		dns[key] = resolvers
	}
	dns["fake-ip-filter"] = []string{"localhost"}
	dns["use-system-hosts"] = false
	cfg["sniffer"] = Object{"enable": false}
	return cfg, nil
}

func makeP2PConfig(network string, node, settings Object, platform string, fd int) (Object, error) {
	cfg, err := makeTorConfig(node, settings, platform, fd)
	if err != nil || network != "i2p" {
		return cfg, err
	}
	cfg["rules"] = []string{"NETWORK,udp,REJECT", "DOMAIN-SUFFIX,i2p,VPN", "MATCH,REJECT"}
	dns := obj(cfg["dns"])
	for _, key := range []string{"nameserver", "default-nameserver", "proxy-server-nameserver", "direct-nameserver"} {
		dns[key] = []string{"rcode://success"}
	}
	dns["ipv6"] = false
	// Mihomo requires literal bootstrap resolvers. No configured I2P resolver
	// needs DNS bootstrap; a discard port on loopback makes fallback fail closed.
	dns["default-nameserver"] = []string{"127.0.0.1:1"}
	return cfg, nil
}

// Only the trusted Android JNI boundary can supply the packaged helper path.
func (c *Controller) SetTorHelperPath(path string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.platform == "android" && filepath.IsAbs(path) && filepath.Base(path) == "libmukhomor_tor.so" {
		c.torHelper = path
	}
}
