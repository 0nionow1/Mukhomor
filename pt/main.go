// Private client-only bridge adapter. The parent owns its lifetime through stdin.
// No relaying, user-supplied executables, command lines, or public listeners.
package main

import (
	"bufio"
	"bytes"
	"crypto/subtle"
	"encoding/json"
	"errors"
	"io"
	"log"
	"net"
	"net/netip"
	"net/url"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	pt "gitlab.torproject.org/tpo/anti-censorship/pluggable-transports/goptlib"
	"gitlab.torproject.org/tpo/anti-censorship/pluggable-transports/lyrebird/transports/base"
	"gitlab.torproject.org/tpo/anti-censorship/pluggable-transports/lyrebird/transports/obfs4"
	"gitlab.torproject.org/tpo/anti-censorship/pluggable-transports/lyrebird/transports/snowflake"
	"gitlab.torproject.org/tpo/anti-censorship/pluggable-transports/lyrebird/transports/webtunnel"
	sfproxy "gitlab.torproject.org/tpo/anti-censorship/pluggable-transports/snowflake/v2/common/proxy"
)

type initialization struct {
	StateDir  string   `json:"state_dir"`
	Interface string   `json:"interface"`
	Token     string   `json:"token"`
	Password  string   `json:"password,omitempty"`
	Mode      string   `json:"mode"`
	Bridges   []string `json:"bridges"`
}
type bridge struct {
	address string
	args    pt.Args
}

func parseBridge(mode, line string) (bridge, error) {
	fail := errors.New("Invalid Tor bridge configuration")
	fields := strings.Fields(strings.TrimPrefix(strings.TrimSpace(line), "Bridge "))
	if len(fields) < 3 || fields[0] != mode || len(fields[2]) != 40 {
		return bridge{}, fail
	}
	for _, c := range fields[2] {
		if !strings.ContainsRune("0123456789abcdefABCDEF", c) {
			return bridge{}, fail
		}
	}
	address, err := netip.ParseAddrPort(fields[1])
	if err != nil || address.Port() == 0 || address.Addr().Zone() != "" {
		return bridge{}, fail
	}
	b := bridge{address: address.String(), args: pt.Args{}}
	for _, f := range fields[3:] {
		k, v, ok := strings.Cut(f, "=")
		if !ok || v == "" || strings.ContainsAny(f, "\\;\x00\r\n") || len(b.args[k]) != 0 {
			return bridge{}, fail
		}
		allowed := (mode == "obfs4" && (k == "cert" || k == "iat-mode")) || (mode == "webtunnel" && (k == "url" || k == "ver" || k == "utls" || k == "cert")) || (mode == "snowflake" && (k == "url" || k == "ampcache" || k == "front" || k == "fronts" || k == "ice" || k == "utls-imitate" || k == "fingerprint"))
		if !allowed {
			return bridge{}, fail
		}
		b.args.Add(k, v)
	}
	if mode == "webtunnel" || mode == "snowflake" {
		for _, key := range []string{"url", "ampcache"} {
			raw, present := b.args.Get(key)
			if key == "ampcache" && !present {
				continue
			}
			u, e := url.Parse(raw)
			if e != nil || u.Scheme != "https" || u.Hostname() == "" || u.User != nil || u.Fragment != "" {
				return bridge{}, fail
			}
		}
	}
	return b, nil
}

func run() error {
	log.SetOutput(io.Discard)
	pt.Stdout = io.Discard
	// Third-party PT status messages may contain bridge addresses. Keep stdout
	// private to our JSON protocol, and never write their logs to a file.
	protocol := os.Stdout
	defer runtime.KeepAlive(protocol) // Retain the ownership pipe after its ready frame.
	null, err := os.OpenFile(os.DevNull, os.O_WRONLY, 0)
	if err != nil {
		return err
	}
	os.Stdout = null
	os.Stderr = null
	input := bufio.NewReader(io.LimitReader(os.Stdin, 32769))
	line, err := input.ReadBytes('\n')
	if err != nil || len(line) > 32768 {
		return errors.New("PT_INIT_FAILED")
	}
	var cfg initialization
	decoder := json.NewDecoder(bytes.NewReader(line))
	decoder.DisallowUnknownFields()
	if decoder.Decode(&cfg) != nil || len(cfg.Token) != 64 || !filepath.IsAbs(cfg.StateDir) || (cfg.Mode != "i2p" && len(cfg.Bridges) < 1) || len(cfg.Bridges) > 16 {
		return errors.New("PT_INIT_FAILED")
	}
	// Buffered bytes after the single frame also terminate the process.
	go func() {
		var b [1]byte
		if input.Buffered() > 0 {
			os.Exit(0)
		}
		_, _ = os.Stdin.Read(b[:])
		os.Exit(0)
	}()
	physical := newUnderlay(cfg.Interface)
	// Never let a library silently issue system DNS outside the bound DoH path.
	disableSystemDNS()
	if cfg.Mode == "i2p" {
		return runI2P(cfg, physical, protocol)
	}
	var transport base.Transport
	switch cfg.Mode {
	case "obfs4":
		transport = &obfs4.Transport{}
	case "webtunnel":
		transport = webtunnel.Transport
	case "snowflake":
		transport = &snowflake.Transport{}
	default:
		return errors.New("PT_MODE_INVALID")
	}
	if cfg.Mode == "snowflake" {
		sfproxy.SetLocalNetwork(physical)
	}
	proxyURL := ""
	if cfg.Mode == "snowflake" {
		proxyURL, err = startBrokerProxy(physical, cfg.Token)
		if err != nil {
			return err
		}
	}
	ports := []int{}
	slots := make(chan struct{}, 32)
	for _, line := range cfg.Bridges {
		b, e := parseBridge(cfg.Mode, line)
		if e != nil {
			return e
		}
		if cfg.Mode == "snowflake" {
			b.args.Add("proxy", proxyURL)
		}
		factory, e := transport.ClientFactory(cfg.StateDir)
		if e != nil {
			return e
		}
		args, e := factory.ParseArgs(&b.args)
		if e != nil {
			return errors.New("PT_BRIDGE_INVALID")
		}
		ln, e := pt.ListenSocks("tcp4", "127.0.0.1:0")
		if e != nil {
			return e
		}
		ports = append(ports, ln.Addr().(*net.TCPAddr).Port)
		go func() {
			for {
				c, e := ln.AcceptSocks()
				if e != nil {
					if errors.Is(e, net.ErrClosed) {
						return
					}
					continue // A malformed local client must not disable the listener.
				}
				select {
				case slots <- struct{}{}:
					go func() {
						defer func() { <-slots }()
						defer c.Close()
						token, _ := c.Req.Args.Get("mukhomor-token")
						if subtle.ConstantTimeCompare([]byte(token), []byte(cfg.Token)) != 1 || c.Req.Target != b.address {
							_ = c.RejectReason(pt.SocksRepConnectionNotAllowed)
							return
						}
						remote, e := factory.Dial("tcp", b.address, physical.Dial, args)
						if e != nil {
							_ = c.Reject()
							return
						}
						defer remote.Close()
						if c.Grant(nil) != nil {
							return
						}
						relay(c, remote)
					}()
				default:
					_ = c.Reject()
					_ = c.Close()
				}
			}
		}()
	}
	if err = json.NewEncoder(protocol).Encode(map[string]any{"ports": ports}); err != nil {
		return err
	}
	select {}
}
func relay(a, b net.Conn) {
	done := make(chan struct{}, 1)
	go func() { _, _ = io.Copy(a, b); done <- struct{}{} }()
	_, _ = io.Copy(b, a)
	_ = a.Close()
	_ = b.Close()
	<-done
}
func main() {
	defer func() {
		if recover() != nil {
			os.Exit(1)
		}
	}()
	if run() != nil {
		os.Exit(1)
	}
}

// A TCP-only, authenticated local proxy is used for the Snowflake broker's TLS.
// Its UDP uses the injected network directly: there is no unauthenticated UDP
// relay port and no SOCKS5 UDP association whose lifetime can escape the owner.
func startBrokerProxy(n *underlay, token string) (string, error) {
	ln, e := net.Listen("tcp4", "127.0.0.1:0")
	if e != nil {
		return "", e
	}
	slots := make(chan struct{}, 32)
	go func() {
		for {
			c, e := ln.Accept()
			if e != nil {
				return
			}
			select {
			case slots <- struct{}{}:
				go func() {
					defer func() { <-slots }()
					defer c.Close()
					var e error
					_ = c.SetDeadline(time.Now().Add(15 * time.Second))
					b := make([]byte, 2)
					if _, e = io.ReadFull(c, b); e != nil || b[0] != 5 {
						return
					}
					methods := make([]byte, int(b[1]))
					if _, e = io.ReadFull(c, methods); e != nil || !bytes.Contains(methods, []byte{2}) {
						_, _ = c.Write([]byte{5, 255})
						return
					}
					_, _ = c.Write([]byte{5, 2})
					if _, e = io.ReadFull(c, b); e != nil || b[0] != 1 {
						return
					}
					user := make([]byte, int(b[1]))
					if _, e = io.ReadFull(c, user); e != nil {
						return
					}
					if _, e = io.ReadFull(c, b[:1]); e != nil {
						return
					}
					pass := make([]byte, int(b[0]))
					if _, e = io.ReadFull(c, pass); e != nil {
						return
					}
					if subtle.ConstantTimeCompare(user, []byte(token))&subtle.ConstantTimeCompare(pass, []byte(token)) != 1 {
						_, _ = c.Write([]byte{1, 1})
						return
					}
					_, _ = c.Write([]byte{1, 0})
					header := make([]byte, 4)
					if _, e = io.ReadFull(c, header); e != nil || header[0] != 5 || header[1] != 1 || header[2] != 0 {
						return
					}
					host := ""
					switch header[3] {
					case 1:
						b = make([]byte, 4)
						_, e = io.ReadFull(c, b)
						host = net.IP(b).String()
					case 4:
						b = make([]byte, 16)
						_, e = io.ReadFull(c, b)
						host = net.IP(b).String()
					case 3:
						b = make([]byte, 1)
						if _, e = io.ReadFull(c, b); e != nil {
							return
						}
						b = make([]byte, int(b[0]))
						_, e = io.ReadFull(c, b)
						host = string(b)
					default:
						return
					}
					if e != nil {
						return
					}
					b = make([]byte, 2)
					if _, e = io.ReadFull(c, b); e != nil {
						return
					}
					port := int(b[0])<<8 | int(b[1])
					if port != 443 {
						return
					}
					remote, e := n.Dial("tcp", net.JoinHostPort(host, "443"))
					if e != nil {
						return
					}
					defer remote.Close()
					_, _ = c.Write([]byte{5, 0, 0, 1, 0, 0, 0, 0, 0, 0})
					_ = c.SetDeadline(time.Time{})
					relay(c, remote)
				}()
			default:
				_ = c.Close()
			}
		}
	}()
	return "socks5://" + token + ":" + token + "@" + ln.Addr().String(), nil
}
