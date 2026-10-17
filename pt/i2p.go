package main

import (
	"bufio"
	"bytes"
	"context"
	"crypto/subtle"
	"embed"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"time"
)

// Upstream SU3 signer certificates, copied from the SHA-256-pinned i2pd source.
//
//go:embed i2p-reseed/*.crt
var i2pCertificates embed.FS

var i2pSeeds = []string{
	"https://reseed.diva.exchange/i2pseeds.su3",
	"https://reseed.stormycloud.org/i2pseeds.su3",
	"https://reseed-fr.i2pd.xyz/i2pseeds.su3",
	"https://reseed2.i2p.net/i2pseeds.su3",
}

func privateI2PDir(path string) error {
	if err := os.MkdirAll(path, 0700); err != nil {
		return err
	}
	info, err := os.Lstat(path)
	if err != nil || !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
		return errors.New("I2P_STATE_INVALID")
	}
	return nil
}

// This client has no system resolver, proxy environment, redirect, or unbound
// network fallback. i2pd independently verifies the fetched SU3 signature.
func fetchI2PSeed(n *underlay, path string) error {
	transport := &http.Transport{DialContext: func(ctx context.Context, network, address string) (net.Conn, error) {
		return n.Dial(network, address)
	}}
	defer transport.CloseIdleConnections()
	client := &http.Client{Transport: transport, Timeout: 25 * time.Second,
		CheckRedirect: func(*http.Request, []*http.Request) error { return errors.New("I2P_RESEED_REDIRECT_REFUSED") }}
	for _, address := range i2pSeeds {
		response, err := client.Get(address)
		if err != nil {
			continue
		}
		data, err := io.ReadAll(io.LimitReader(response.Body, 4*1024*1024+1))
		response.Body.Close()
		if err != nil || response.StatusCode != 200 || len(data) < 64 || len(data) > 4*1024*1024 || !bytes.HasPrefix(data, []byte("I2Psu3\x00")) {
			continue
		}
		return os.WriteFile(path, data, 0600)
	}
	return errors.New("I2P_RESEED_UNAVAILABLE")
}

// Only authenticated i2pd NTCP2 connections reach this proxy. Hostnames are
// refused: remote routers are public IPs from the signed I2P router database.
func startI2PUnderlay(n *underlay, token string) (net.Listener, error) {
	ln, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		return nil, err
	}
	want := "Basic " + base64.StdEncoding.EncodeToString([]byte(token+":"+token))
	slots := make(chan struct{}, 192)
	go func() {
		for {
			c, err := ln.Accept()
			if err != nil {
				return
			}
			select {
			case slots <- struct{}{}:
				go func() {
					defer func() { <-slots; c.Close() }()
					c.SetDeadline(time.Now().Add(15 * time.Second))
					reader := bufio.NewReader(io.LimitReader(c, 8193))
					r, err := http.ReadRequest(reader)
					if err != nil || r.Method != http.MethodConnect || subtle.ConstantTimeCompare([]byte(r.Header.Get("Proxy-Authorization")), []byte(want)) != 1 {
						return
					}
					host, port, err := net.SplitHostPort(r.RequestURI)
					ip := net.ParseIP(host)
					p, e := strconv.Atoi(port)
					if err != nil || e != nil || p < 1 || p > 65535 || p == 53 || p == 853 || ip == nil || !publicIP(ip) || r.ContentLength > 0 || len(r.TransferEncoding) != 0 {
						return
					}
					remote, err := n.Dial("tcp", net.JoinHostPort(host, port))
					if err != nil {
						return
					}
					defer remote.Close()
					if _, err = io.WriteString(c, "HTTP/1.1 200 Connection Established\r\n\r\n"); err != nil {
						return
					}
					// Bytes after the bounded CONNECT header belong to NTCP2.
					if reader.Buffered() > 0 {
						b, _ := reader.Peek(reader.Buffered())
						if _, err = remote.Write(b); err != nil {
							return
						}
					}
					c.SetDeadline(time.Time{})
					relay(c, remote)
				}()
			default:
				c.Close()
			}
		}
	}()
	return ln, nil
}

func validI2PHost(host string) bool {
	if len(host) > 253 || !strings.HasSuffix(host, ".i2p") || host == ".i2p" {
		return false
	}
	for _, label := range strings.Split(host, ".") {
		if len(label) == 0 || len(label) > 63 || label[0] == '-' || label[len(label)-1] == '-' {
			return false
		}
		for _, c := range label {
			if !(c >= 'a' && c <= 'z' || c >= '0' && c <= '9' || c == '-') {
				return false
			}
		}
	}
	return true
}

func i2pRequest(c net.Conn, username, password string) (string, uint16, error) {
	fail := errors.New("I2P_DESTINATION_REFUSED")
	var b [4]byte
	if _, e := io.ReadFull(c, b[:2]); e != nil || b[0] != 5 {
		return "", 0, fail
	}
	methods := make([]byte, int(b[1]))
	if _, e := io.ReadFull(c, methods); e != nil || !bytes.Contains(methods, []byte{2}) {
		c.Write([]byte{5, 255})
		return "", 0, fail
	}
	c.Write([]byte{5, 2})
	if _, e := io.ReadFull(c, b[:2]); e != nil || b[0] != 1 {
		return "", 0, fail
	}
	user := make([]byte, int(b[1]))
	if _, e := io.ReadFull(c, user); e != nil {
		return "", 0, fail
	}
	if _, e := io.ReadFull(c, b[:1]); e != nil {
		return "", 0, fail
	}
	pass := make([]byte, int(b[0]))
	if _, e := io.ReadFull(c, pass); e != nil {
		return "", 0, fail
	}
	if subtle.ConstantTimeCompare(user, []byte(username))&subtle.ConstantTimeCompare(pass, []byte(password)) != 1 {
		c.Write([]byte{1, 1})
		return "", 0, fail
	}
	c.Write([]byte{1, 0})
	if _, e := io.ReadFull(c, b[:]); e != nil || b[0] != 5 || b[1] != 1 || b[2] != 0 || b[3] != 3 {
		return "", 0, fail
	}
	if _, e := io.ReadFull(c, b[:1]); e != nil {
		return "", 0, fail
	}
	name := make([]byte, int(b[0]))
	if _, e := io.ReadFull(c, name); e != nil {
		return "", 0, fail
	}
	if _, e := io.ReadFull(c, b[:2]); e != nil {
		return "", 0, fail
	}
	host := strings.ToLower(string(name))
	port := uint16(b[0])<<8 | uint16(b[1])
	if !validI2PHost(host) || port == 0 {
		return "", 0, fail
	}
	return host, port, nil
}

func serveI2P(ln net.Listener, port int, cfg initialization) {
	slots := make(chan struct{}, 64)
	for {
		c, err := ln.Accept()
		if err != nil {
			return
		}
		select {
		case slots <- struct{}{}:
			go func() {
				defer func() { <-slots; c.Close() }()
				c.SetDeadline(time.Now().Add(15 * time.Second))
				host, p, err := i2pRequest(c, cfg.Token, cfg.Password)
				fail := []byte{5, 2, 0, 1, 0, 0, 0, 0, 0, 0}
				if err != nil {
					c.Write(fail)
					return
				}
				c.SetDeadline(time.Now().Add(100 * time.Second))
				remote, err := net.DialTimeout("tcp4", net.JoinHostPort("127.0.0.1", strconv.Itoa(port)), 5*time.Second)
				if err != nil {
					c.Write(fail)
					return
				}
				defer remote.Close()
				remote.SetDeadline(time.Now().Add(90 * time.Second))
				remote.Write(append([]byte(cfg.Token), 5, 1, 0))
				var b [4]byte
				if _, err = io.ReadFull(remote, b[:2]); err != nil || b[0] != 5 || b[1] != 0 {
					c.Write(fail)
					return
				}
				request := append([]byte{5, 1, 0, 3, byte(len(host))}, []byte(host)...)
				request = append(request, byte(p>>8), byte(p))
				remote.Write(request)
				if _, err = io.ReadFull(remote, b[:]); err != nil || b[0] != 5 || b[1] != 0 || b[2] != 0 {
					c.Write(fail)
					return
				}
				size := 0
				switch b[3] {
				case 1:
					size = 4
				case 4:
					size = 16
				case 3:
					if _, err = io.ReadFull(remote, b[:1]); err != nil {
						return
					}
					size = int(b[0])
				default:
					return
				}
				if _, err = io.CopyN(io.Discard, remote, int64(size+2)); err != nil {
					return
				}
				c.SetDeadline(time.Time{})
				remote.SetDeadline(time.Time{})
				c.Write([]byte{5, 0, 0, 1, 0, 0, 0, 0, 0, 0})
				relay(c, remote)
			}()
		default:
			c.Close()
		}
	}
}

func runI2P(cfg initialization, n *underlay, protocol io.Writer) error {
	if len(cfg.Password) != 64 || len(cfg.Bridges) != 0 || strings.ContainsAny(cfg.StateDir, "\r\n\x00") || strings.Trim(cfg.Token, "0123456789abcdef") != "" {
		return errors.New("I2P_INIT_INVALID")
	}
	if err := privateI2PDir(cfg.StateDir); err != nil {
		return err
	}
	session, err := os.MkdirTemp(cfg.StateDir, "session-")
	if err != nil {
		return err
	}
	defer os.RemoveAll(session)
	certs := filepath.Join(session, "certificates", "reseed")
	if err = privateI2PDir(certs); err != nil {
		return err
	}
	entries, _ := i2pCertificates.ReadDir("i2p-reseed")
	for _, entry := range entries {
		data, e := i2pCertificates.ReadFile("i2p-reseed/" + entry.Name())
		if e != nil {
			return e
		}
		if e = os.WriteFile(filepath.Join(certs, entry.Name()), data, 0600); e != nil {
			return e
		}
	}
	seed := filepath.Join(session, "bootstrap.su3")
	if err = fetchI2PSeed(n, seed); err != nil {
		return err
	}
	underlay, err := startI2PUnderlay(n, cfg.Token)
	if err != nil {
		return err
	}
	defer underlay.Close()
	config := fmt.Sprintf(`ipv4=true
ipv6=false
notransit=true
floodfill=false
share=0
bandwidth=L
loglevel=none
[ntcp2]
enabled=true
published=false
proxy=http://%s:%s@%s
[ssu2]
enabled=false
[http]
enabled=false
[httpproxy]
enabled=false
[socksproxy]
enabled=true
address=127.0.0.1
port=0
outproxy.enabled=false
inbound.length=3
outbound.length=3
[sam]
enabled=false
[bob]
enabled=false
[i2cp]
enabled=false
[i2pcontrol]
enabled=false
[upnp]
enabled=false
[nettime]
enabled=false
[reseed]
verify=true
file=%s
urls=
yggurls=
[addressbook]
enabled=true
subscriptions=
`, cfg.Token, cfg.Token, underlay.Addr().String(), seed)
	conf := filepath.Join(session, "i2pd.conf")
	if err = os.WriteFile(conf, []byte(config), 0600); err != nil {
		return err
	}
	state := filepath.Join(cfg.StateDir, "router")
	if err = privateI2PDir(state); err != nil {
		return err
	}
	// Empty tunnel configuration: no incoming application services.
	tunnelsDir := filepath.Join(session, "tunnels.d")
	if err = privateI2PDir(tunnelsDir); err != nil {
		return err
	}
	tunnels := filepath.Join(session, "tunnels.conf")
	if err = os.WriteFile(tunnels, nil, 0600); err != nil {
		return err
	}
	exe, err := os.Executable()
	if err != nil {
		return err
	}
	name := "mukhomor-i2pd"
	if runtime.GOOS == "windows" {
		name += ".exe"
	}
	if runtime.GOOS == "android" {
		name = "libmukhomor_i2pd.so"
	}
	path := filepath.Join(filepath.Dir(exe), name)
	info, err := os.Lstat(path)
	if err != nil || !info.Mode().IsRegular() {
		return errors.New("I2P_HELPER_MISSING")
	}
	child := exec.Command(path, "--conf="+conf, "--datadir="+state, "--certsdir="+filepath.Dir(certs), "--tunconf="+tunnels, "--tunnelsdir="+tunnelsDir)
	hideI2PChild(child)
	input, err := child.StdinPipe()
	if err != nil {
		return err
	}
	defer input.Close()
	output, err := child.StdoutPipe()
	if err != nil {
		return err
	}
	child.Stderr = io.Discard
	if err = child.Start(); err != nil {
		return errors.New("I2P_START_FAILED")
	}
	defer func() { input.Close(); child.Process.Kill(); child.Wait() }()
	if _, err = io.WriteString(input, cfg.Token+"\n"); err != nil {
		return err
	}
	lines := make(chan []byte, 2)
	go func() {
		defer close(lines)
		scan := bufio.NewScanner(output)
		scan.Buffer(make([]byte, 4096), 4096)
		for scan.Scan() {
			lines <- append([]byte(nil), scan.Bytes()...)
		}
	}()
	timer := time.NewTimer(6 * time.Minute)
	defer timer.Stop()
	port := 0
	for {
		select {
		case line, ok := <-lines:
			if !ok {
				return errors.New("I2P_TRANSPORT_STOPPED")
			}
			var event struct {
				Started bool `json:"started"`
				Ready   bool `json:"ready"`
				Port    int  `json:"port"`
			}
			if json.Unmarshal(line, &event) != nil {
				return errors.New("I2P_PROTOCOL_INVALID")
			}
			if event.Started {
				if event.Port < 1 || event.Port > 65535 || port != 0 {
					return errors.New("I2P_PROTOCOL_INVALID")
				}
				port = event.Port
				// Configuration is loaded before Started; discard its session secret.
				if err = os.Remove(conf); err != nil {
					return errors.New("I2P_CONFIG_CLEANUP_FAILED")
				}
				continue
			}
			if !event.Ready || port == 0 {
				return errors.New("I2P_PROTOCOL_INVALID")
			}
			ln, err := net.Listen("tcp4", "127.0.0.1:0")
			if err != nil {
				return err
			}
			defer ln.Close()
			go serveI2P(ln, port, cfg)
			if err = json.NewEncoder(protocol).Encode(map[string]any{"ready": true, "port": ln.Addr().(*net.TCPAddr).Port}); err != nil {
				return err
			}
			// EOF/error from the native child closes the public SOCKS endpoint.
			<-lines
			return errors.New("I2P_TRANSPORT_STOPPED")
		case <-timer.C:
			return errors.New("I2P_BOOTSTRAP_TIMEOUT")
		}
	}
}
