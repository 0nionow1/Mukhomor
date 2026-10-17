package main

import (
	"context"
	"crypto/tls"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"gitlab.torproject.org/tpo/anti-censorship/pluggable-transports/lyrebird/transports/base"
	"gitlab.torproject.org/tpo/anti-censorship/pluggable-transports/lyrebird/transports/snowflake"
	sfproxy "gitlab.torproject.org/tpo/anti-censorship/pluggable-transports/snowflake/v2/common/proxy"
	"io"
	"log"
	"net"
	"net/url"
	"os"
	"os/exec"
	"runtime"
	"strings"
	"testing"
	"time"
)

func TestPTLifetimeAndMalformedClientIsolation(t *testing.T) {
	if os.Getenv("MUKHOMOR_PT_TEST_CHILD") == "1" {
		go func() {
			for {
				time.Sleep(10 * time.Millisecond)
				runtime.GC()
			}
		}()
		if run() != nil {
			os.Exit(7)
		}
		os.Exit(8)
	}
	cmd := exec.Command(os.Args[0], "-test.run=^TestPTLifetimeAndMalformedClientIsolation$")
	cmd.Env = append(os.Environ(), "MUKHOMOR_PT_TEST_CHILD=1")
	in, _ := cmd.StdinPipe()
	out, _ := cmd.StdoutPipe()
	cmd.Stderr = io.Discard
	if e := cmd.Start(); e != nil {
		t.Fatal(e)
	}
	defer cmd.Process.Kill()
	cert := strings.TrimRight(base64.StdEncoding.EncodeToString(make([]byte, 52)), "=")
	cfg := initialization{StateDir: t.TempDir(), Interface: "7", Token: strings.Repeat("a", 64), Mode: "obfs4", Bridges: []string{"obfs4 192.0.2.1:443 " + strings.Repeat("A", 40) + " cert=" + cert + " iat-mode=0"}}
	if e := json.NewEncoder(in).Encode(cfg); e != nil {
		t.Fatal(e)
	}
	var ready struct {
		Ports []int `json:"ports"`
	}
	if e := json.NewDecoder(out).Decode(&ready); e != nil || len(ready.Ports) != 1 {
		t.Fatal("PT startup", e)
	}
	address := fmt.Sprintf("127.0.0.1:%d", ready.Ports[0])
	c, e := net.DialTimeout("tcp", address, time.Second)
	if e != nil {
		t.Fatal(e)
	}
	c.Write([]byte{1, 1, 0})
	c.Close()
	c, e = net.DialTimeout("tcp", address, time.Second)
	if e != nil {
		t.Fatal(e)
	}
	c.SetDeadline(time.Now().Add(time.Second))
	c.Write([]byte{5, 1, 2})
	reply := make([]byte, 2)
	_, e = io.ReadFull(c, reply)
	c.Close()
	if e != nil || reply[0] != 5 || reply[1] != 2 {
		t.Fatal("Malformed client disabled PT listener", e)
	}
	eof := make(chan error, 1)
	go func() { var b [1]byte; _, e := out.Read(b[:]); eof <- e }()
	select {
	case e := <-eof:
		t.Fatal("Ownership pipe closed while process is alive", e)
	case <-time.After(300 * time.Millisecond):
	}
	in.Close()
	wait := make(chan error, 1)
	go func() { wait <- cmd.Wait() }()
	select {
	case e := <-wait:
		if e != nil {
			t.Fatal(e)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("PT survived parent EOF")
	}
}

func TestExplicitSnowflakeInitialization(t *testing.T) {
	iface := os.Getenv("MUKHOMOR_TEST_INTERFACE")
	if iface == "" {
		t.Skip("explicit socket binding probe")
	}
	n := newUnderlay(iface)
	sfproxy.SetLocalNetwork(n)
	raw, e := startBrokerProxy(n, strings.Repeat("a", 64))
	if e != nil {
		t.Fatal(e)
	}
	b, e := parseBridge("snowflake", "snowflake 192.0.2.3:80 2B280B23E1107BB62ABFC40DDCC8824814F80A72 fingerprint=2B280B23E1107BB62ABFC40DDCC8824814F80A72 url=https://1098762253.rsc.cdn77.org front=www.phpmyadmin.net,cdn.zk.mk ice=stun:stun.nextcloud.com:443 utls-imitate=hellorandomizedalpn")
	if e != nil {
		t.Fatal(e)
	}
	b.args.Add("proxy", raw)
	f, e := (&snowflake.Transport{}).ClientFactory(t.TempDir())
	if e != nil {
		t.Fatal(e)
	}
	if _, e = f.ParseArgs(&b.args); e != nil {
		t.Fatal(e)
	}
}

func TestExplicitSnowflakeConnection(t *testing.T) {
	iface := os.Getenv("MUKHOMOR_TEST_INTERFACE")
	if iface == "" {
		t.Skip("explicit public Snowflake connection probe")
	}
	log.SetOutput(io.Discard)
	previousResolver := net.DefaultResolver
	disableSystemDNS()
	t.Cleanup(func() { net.DefaultResolver = previousResolver })
	n := newUnderlay(iface)
	sfproxy.SetLocalNetwork(n)
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	if _, e := n.resolve(ctx, "stun.nextcloud.com"); e != nil {
		t.Fatal("Infrastructure DNS:", e)
	}
	t.Log("Infrastructure DoH works")
	raw, e := startBrokerProxy(n, strings.Repeat("a", 64))
	if e != nil {
		t.Fatal(e)
	}
	b, e := parseBridge("snowflake", "snowflake 192.0.2.3:80 2B280B23E1107BB62ABFC40DDCC8824814F80A72 fingerprint=2B280B23E1107BB62ABFC40DDCC8824814F80A72 url=https://1098762253.rsc.cdn77.org front=www.phpmyadmin.net,cdn.zk.mk ice=stun:stun.antisip.com:3478,stun:stun.nextcloud.com:443 utls-imitate=hellorandomizedalpn")
	if e != nil {
		t.Fatal(e)
	}
	b.args.Add("proxy", raw)
	f, e := (&snowflake.Transport{}).ClientFactory(t.TempDir())
	if e != nil {
		t.Fatal(e)
	}
	args, e := f.ParseArgs(&b.args)
	if e != nil {
		t.Fatal(e)
	}
	f.OnEvent(func(event base.TransportEvent) { fmt.Printf("Snowflake event: %T\n", event) })
	completed := make(chan error, 1)
	go func() {
		c, e := f.Dial("tcp", b.address, n.Dial, args)
		if e == nil {
			defer c.Close()
			_ = c.SetDeadline(time.Now().Add(55 * time.Second))
			e = tls.Client(c, &tls.Config{ServerName: "bridge.invalid", MinVersion: tls.VersionTLS12}).Handshake()
		}
		completed <- e
	}()
	select {
	case e := <-completed:
		var certificateError *tls.CertificateVerificationError
		if !errors.As(e, &certificateError) {
			t.Fatal("Bridge TLS response was not received:", e)
		}
		t.Log("Bridge TLS certificate received; Arti separately verifies Tor relay identity")
	case <-time.After(60 * time.Second):
		t.Fatal("Snowflake connection timed out")
	}
}

func TestBridgeInputRejectsUnsafeParameters(t *testing.T) {
	prefix := "webtunnel 192.0.2.1:443 0123456789012345678901234567890123456789 "
	for _, tail := range []string{"url=http://example.org/", "url=https://user:pass@example.org/", "url=https://example.org/ proxy=socks5://evil", "url=https://example.org/ url=https://other.example/", "url=https://example.org/ cert-domain=attacker.example"} {
		if _, e := parseBridge("webtunnel", prefix+tail); e == nil {
			t.Fatal("accepted unsafe bridge option")
		}
	}
	if _, e := parseBridge("webtunnel", prefix+"url=https://example.org/private ver=0.0.1"); e != nil {
		t.Fatal(e)
	}
	cert := base64.RawStdEncoding.EncodeToString(make([]byte, 52))
	if _, e := parseBridge("obfs4", "obfs4 192.0.2.1:443 0123456789012345678901234567890123456789 cert="+cert+" iat-mode=0"); e != nil {
		t.Fatal(e)
	}
}

func TestSnowflakeAMPUsesUpstreamTransport(t *testing.T) {
	line := "snowflake 192.0.2.5:80 2B280B23E1107BB62ABFC40DDCC8824814F80A72 fingerprint=2B280B23E1107BB62ABFC40DDCC8824814F80A72 url=https://snowflake-broker.torproject.net/ ampcache=https://cdn.ampproject.org/ front=www.google.com ice=stun:stun.nextcloud.com:443 utls-imitate=hellorandomizedalpn"
	b, err := parseBridge("snowflake", line)
	if err != nil {
		t.Fatal(err)
	}
	factory, err := (&snowflake.Transport{}).ClientFactory(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	if _, err = factory.ParseArgs(&b.args); err != nil {
		t.Fatal("upstream rejected AMP preset", err)
	}
	for _, bad := range []string{"http://cdn.ampproject.org/", "https://user:pass@cdn.ampproject.org/"} {
		if _, err := parseBridge("snowflake", strings.Replace(line, "https://cdn.ampproject.org/", bad, 1)); err == nil {
			t.Fatal("accepted unsafe AMP URL")
		}
	}
}
func TestInfrastructureProxyRequiresSecretAndRejectsUDP(t *testing.T) {
	token := strings.Repeat("a", 64)
	raw, e := startBrokerProxy(newUnderlay("0"), token)
	if e != nil {
		t.Fatal(e)
	}
	u, _ := url.Parse(raw)
	for _, authenticated := range []bool{false, true} {
		c, e := net.DialTimeout("tcp", u.Host, time.Second)
		if e != nil {
			t.Fatal(e)
		}
		defer c.Close()
		_ = c.SetDeadline(time.Now().Add(time.Second))
		if !authenticated {
			_, _ = c.Write([]byte{5, 1, 0})
			b := make([]byte, 2)
			_, _ = io.ReadFull(c, b)
			if b[1] != 255 {
				t.Fatal("unauthenticated proxy")
			}
			continue
		}
		_, _ = c.Write([]byte{5, 1, 2})
		b := make([]byte, 2)
		if _, e = io.ReadFull(c, b); e != nil || b[1] != 2 {
			t.Fatal("authentication negotiation")
		}
		frame := append([]byte{1, 64}, []byte(token)...)
		frame = append(frame, 64)
		frame = append(frame, []byte(token)...)
		_, _ = c.Write(frame)
		_, e = io.ReadFull(c, b)
		if e != nil || b[1] != 0 {
			t.Fatal("authentication rejected")
		}
		_, _ = c.Write([]byte{5, 3, 0, 1, 0, 0, 0, 0, 0, 0})
		if _, e = c.Read(b); e == nil {
			t.Fatal("UDP association accepted")
		}
	}
}
func TestUnderlayRejectsLocalAddressesAndDNSPorts(t *testing.T) {
	for _, ip := range []string{"127.0.0.1", "::1", "0.0.0.0", "10.2.3.4", "192.168.1.1", "169.254.1.1", "fc00::1", "ff02::1", "100.64.1.1", "198.18.0.1", "240.0.0.1", "2001:db8::1"} {
		if publicIP(net.ParseIP(ip)) {
			t.Fatal("private underlay destination accepted")
		}
	}
	for _, a := range []*net.UDPAddr{{IP: net.ParseIP("1.1.1.1"), Port: 53}, {IP: net.ParseIP("8.8.8.8"), Port: 853}, {IP: net.ParseIP("127.0.0.1"), Port: 443}} {
		if allowedUDP(a) {
			t.Fatal("unsafe UDP allowed")
		}
	}
	if !allowedUDP(&net.UDPAddr{IP: net.ParseIP("1.1.1.1"), Port: 3478}) {
		t.Fatal("STUN unexpectedly blocked")
	}
	n := newUnderlay("0")
	for _, address := range []string{"1.1.1.1:53", "1.1.1.1:853", "127.0.0.1:443", "[::1]:443"} {
		if c, e := n.Dial("tcp", address); e == nil {
			c.Close()
			t.Fatal("unsafe TCP allowed")
		}
	}
}
