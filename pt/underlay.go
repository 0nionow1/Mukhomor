package main

import (
	"bytes"
	"context"
	"errors"
	"io"
	"net"
	"net/http"
	"net/netip"
	"runtime"
	"strconv"
	"strings"
	"sync/atomic"
	"time"

	"github.com/miekg/dns"
	"github.com/pion/transport/v4"
	"github.com/pion/transport/v4/stdnet"
)

const tcpMark = 0x4d544f52
const udpMark = 0x4d505455

type underlay struct {
	base         transport.Net
	iface        string
	dns          []dohResolver
	preferredDNS atomic.Uint32
}

type dohResolver struct {
	url    string
	client *http.Client
}

func newUnderlay(iface string) *underlay {
	base, _ := stdnet.NewNet()
	n := &underlay{base: base, iface: iface}
	// Only transport infrastructure names use these resolvers. User DNS stays
	// inside Tor/I2P. A blocked resolver must not disable every bridge/reseed.
	for _, endpoint := range []struct{ url, ip string }{
		{"https://cloudflare-dns.com/dns-query", "1.1.1.1:443"},
		{"https://dns.google/dns-query", "8.8.8.8:443"},
	} {
		n.dns = append(n.dns, dohResolver{endpoint.url, &http.Client{Timeout: 5 * time.Second,
			CheckRedirect: func(*http.Request, []*http.Request) error { return errors.New("DNS redirect refused") },
			Transport: &http.Transport{Proxy: nil, ForceAttemptHTTP2: true, TLSHandshakeTimeout: 4 * time.Second, MaxResponseHeaderBytes: 8192,
				DialContext: func(ctx context.Context, network, address string) (net.Conn, error) {
					return n.dialIP(ctx, "tcp4", endpoint.ip)
				},
			},
		}})
	}
	return n
}
func disableSystemDNS() {
	net.DefaultResolver = &net.Resolver{PreferGo: true, Dial: func(context.Context, string, string) (net.Conn, error) {
		return nil, errors.New("System DNS disabled for Tor bridges")
	}}
}
func publicIP(ip net.IP) bool {
	if ip == nil || !ip.IsGlobalUnicast() || ip.IsPrivate() || ip.IsLoopback() || ip.IsLinkLocalUnicast() || ip.IsUnspecified() {
		return false
	}
	a, ok := netip.AddrFromSlice(ip)
	if !ok {
		return false
	}
	a = a.Unmap()
	for _, prefix := range nonPublicRanges {
		if prefix.Contains(a) {
			return false
		}
	}
	return true
}

var nonPublicRanges = []netip.Prefix{
	netip.MustParsePrefix("0.0.0.0/8"), netip.MustParsePrefix("100.64.0.0/10"),
	netip.MustParsePrefix("192.0.0.0/24"), netip.MustParsePrefix("192.0.2.0/24"),
	netip.MustParsePrefix("198.18.0.0/15"), netip.MustParsePrefix("198.51.100.0/24"),
	netip.MustParsePrefix("203.0.113.0/24"), netip.MustParsePrefix("240.0.0.0/4"),
	netip.MustParsePrefix("2001:db8::/32"), netip.MustParsePrefix("100::/64"),
}

func (n *underlay) resolve(ctx context.Context, host string) (net.IP, error) {
	if ip := net.ParseIP(host); ip != nil {
		if publicIP(ip) {
			return ip, nil
		}
		return nil, errors.New("Non-public bridge destination")
	}
	if len(host) > 253 || strings.ContainsAny(host, "\x00\r\n /\\%@") {
		return nil, errors.New("Invalid bridge hostname")
	}
	m := new(dns.Msg)
	m.SetQuestion(dns.Fqdn(host), dns.TypeA)
	m.RecursionDesired = true
	wire, e := m.Pack()
	if e != nil {
		return nil, e
	}
	start := int(n.preferredDNS.Load())
	for offset := range n.dns {
		if ctx.Err() != nil {
			break
		}
		index := (start + offset) % len(n.dns)
		ip, err := n.dns[index].resolve(ctx, m, wire)
		if err == nil {
			n.preferredDNS.Store(uint32(index))
			return ip, nil
		}
	}
	return nil, errors.New("Bridge DNS unavailable; encrypted resolvers did not return a public address")
}

func (resolver dohResolver) resolve(ctx context.Context, m *dns.Msg, wire []byte) (net.IP, error) {
	request, e := http.NewRequestWithContext(ctx, http.MethodPost, resolver.url, bytes.NewReader(wire))
	if e != nil {
		return nil, e
	}
	request.Header.Set("Content-Type", "application/dns-message")
	request.Header.Set("Accept", "application/dns-message")
	request.Header.Set("User-Agent", "")
	response, e := resolver.client.Do(request)
	if e != nil {
		return nil, errors.New("Bridge DNS unavailable")
	}
	defer response.Body.Close()
	if response.StatusCode != 200 || strings.Split(response.Header.Get("Content-Type"), ";")[0] != "application/dns-message" {
		return nil, errors.New("Bridge DNS unavailable")
	}
	wire, e = io.ReadAll(io.LimitReader(response.Body, 65537))
	if e != nil || len(wire) > 65536 {
		return nil, errors.New("Bridge DNS response invalid")
	}
	answer := new(dns.Msg)
	if answer.Unpack(wire) != nil || !answer.Response || answer.Truncated || answer.Id != m.Id || answer.Rcode != dns.RcodeSuccess || len(answer.Question) != 1 || answer.Question[0] != m.Question[0] {
		return nil, errors.New("Bridge DNS response invalid")
	}
	// Accept only the requested name or a bounded CNAME chain, never an
	// unrelated additional A record injected into a response.
	name := m.Question[0].Name
	for range 16 {
		next := ""
		for _, rr := range answer.Answer {
			if rr.Header().Class != dns.ClassINET || !strings.EqualFold(rr.Header().Name, name) {
				continue
			}
			if a, ok := rr.(*dns.A); ok && publicIP(a.A) {
				return a.A, nil
			}
			if c, ok := rr.(*dns.CNAME); ok {
				next = c.Target
			}
		}
		if next == "" {
			break
		}
		name = next
	}
	return nil, errors.New("No public bridge address")
}
func (n *underlay) dialIP(ctx context.Context, network, address string) (net.Conn, error) {
	return (&net.Dialer{Timeout: 15 * time.Second, KeepAlive: 30 * time.Second, Control: n.control}).DialContext(ctx, network, address)
}
func (n *underlay) Dial(network, address string) (net.Conn, error) {
	if network != "tcp" && network != "tcp4" && network != "tcp6" {
		return nil, errors.New("TCP required")
	}
	host, p, e := net.SplitHostPort(address)
	if e != nil {
		return nil, e
	}
	port, e := strconv.Atoi(p)
	if e != nil || port < 1 || port > 65535 || port == 53 || port == 853 {
		return nil, errors.New("Bridge port refused")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	ip, e := n.resolve(ctx, host)
	if e != nil {
		return nil, e
	}
	family := "tcp4"
	if ip.To4() == nil {
		family = "tcp6"
	}
	return n.dialIP(ctx, family, net.JoinHostPort(ip.String(), p))
}
func (n *underlay) ResolveUDPAddr(network, address string) (*net.UDPAddr, error) {
	host, p, e := net.SplitHostPort(address)
	if e != nil {
		return nil, e
	}
	port, e := strconv.Atoi(p)
	if e != nil || port < 1 || port > 65535 || port == 53 || port == 853 {
		return nil, errors.New("Bridge UDP port refused")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 12*time.Second)
	defer cancel()
	ip, e := n.resolve(ctx, host)
	if e != nil {
		return nil, e
	}
	return &net.UDPAddr{IP: ip, Port: port}, nil
}

// Only these explicitly bound/marked sockets leave the process. Pion receives
// the same UDPConn interface, without a local SOCKS UDP listener.
func (n *underlay) ListenUDP(network string, local *net.UDPAddr) (transport.UDPConn, error) {
	if network == "udp" {
		network = "udp4"
	}
	if network != "udp4" && network != "udp6" {
		return nil, errors.New("UDP required")
	}
	address := "0.0.0.0:0"
	if network == "udp6" {
		address = "[::]:0"
	}
	if local != nil {
		address = local.String()
	}
	pc, e := (&net.ListenConfig{Control: n.control}).ListenPacket(context.Background(), network, address)
	if e != nil {
		return nil, e
	}
	return &restrictedUDP{UDPConn: pc.(*net.UDPConn)}, nil
}

func (n *underlay) Interfaces() ([]*transport.Interface, error) {
	all, e := n.base.Interfaces()
	if e != nil {
		return nil, e
	}
	selected := []*transport.Interface{}
	for _, iface := range all {
		keep := iface.Name == n.iface
		if runtime.GOOS == "windows" {
			keep = strconv.Itoa(iface.Index) == n.iface
		}
		if runtime.GOOS == "android" {
			keep = !strings.HasPrefix(iface.Name, "tun") && !strings.HasPrefix(iface.Name, "mukhomor")
		}
		if keep && iface.Flags&net.FlagUp != 0 && iface.Flags&net.FlagLoopback == 0 {
			selected = append(selected, iface)
		}
	}
	return selected, nil
}
func (n *underlay) ListenPacket(network, address string) (net.PacketConn, error) {
	host, _, err := net.SplitHostPort(address)
	if err != nil || (host != "" && net.ParseIP(host) == nil) {
		return nil, errors.New("Literal local address required")
	}
	local, e := net.ResolveUDPAddr(network, address)
	if e != nil {
		return nil, e
	}
	return n.ListenUDP(network, local)
}

// Do not inherit stdnet socket factories: a future upstream call must either
// use a bound implementation or fail closed, never silently bypass the tunnel.
func (n *underlay) ListenTCP(string, *net.TCPAddr) (transport.TCPListener, error) {
	return nil, errors.New("Bridge TCP listener refused")
}
func (n *underlay) DialUDP(string, *net.UDPAddr, *net.UDPAddr) (transport.UDPConn, error) {
	return nil, errors.New("Connected bridge UDP refused")
}
func (n *underlay) DialTCP(string, *net.TCPAddr, *net.TCPAddr) (transport.TCPConn, error) {
	return nil, errors.New("Alternate bridge TCP refused")
}
func (n *underlay) ResolveIPAddr(network, address string) (*net.IPAddr, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 12*time.Second)
	defer cancel()
	ip, err := n.resolve(ctx, address)
	if err != nil {
		return nil, err
	}
	return &net.IPAddr{IP: ip}, nil
}
func (n *underlay) ResolveTCPAddr(network, address string) (*net.TCPAddr, error) {
	u, err := n.ResolveUDPAddr(network, address)
	if err != nil {
		return nil, err
	}
	return &net.TCPAddr{IP: u.IP, Port: u.Port}, nil
}
func (n *underlay) InterfaceByIndex(index int) (*transport.Interface, error) {
	all, err := n.Interfaces()
	if err != nil {
		return nil, err
	}
	for _, item := range all {
		if item.Index == index {
			return item, nil
		}
	}
	return nil, errors.New("Physical interface required")
}
func (n *underlay) InterfaceByName(name string) (*transport.Interface, error) {
	all, err := n.Interfaces()
	if err != nil {
		return nil, err
	}
	for _, item := range all {
		if item.Name == name {
			return item, nil
		}
	}
	return nil, errors.New("Physical interface required")
}
func (n *underlay) CreateDialer(*net.Dialer) transport.Dialer { return n }

type boundListener struct{ n *underlay }

func (b boundListener) Listen(context.Context, string, string) (net.Listener, error) {
	return nil, errors.New("Bridge TCP listener refused")
}
func (b boundListener) ListenPacket(_ context.Context, network, address string) (net.PacketConn, error) {
	return b.n.ListenPacket(network, address)
}
func (n *underlay) CreateListenConfig(*net.ListenConfig) transport.ListenConfig {
	return boundListener{n}
}

type restrictedUDP struct{ *net.UDPConn }

func allowedUDP(a *net.UDPAddr) bool {
	return a != nil && publicIP(a.IP) && a.Zone == "" && a.Port > 0 && a.Port != 53 && a.Port != 853
}
func (c *restrictedUDP) WriteToUDP(b []byte, a *net.UDPAddr) (int, error) {
	if !allowedUDP(a) {
		return 0, errors.New("Bridge UDP destination refused")
	}
	return c.UDPConn.WriteToUDP(b, a)
}
func (c *restrictedUDP) WriteTo(b []byte, a net.Addr) (int, error) {
	v, ok := a.(*net.UDPAddr)
	if !ok {
		return 0, errors.New("UDP address required")
	}
	return c.WriteToUDP(b, v)
}
func (c *restrictedUDP) WriteMsgUDP(b, oob []byte, a *net.UDPAddr) (int, int, error) {
	if len(oob) != 0 || !allowedUDP(a) {
		return 0, 0, errors.New("Bridge UDP destination refused")
	}
	return c.UDPConn.WriteMsgUDP(b, nil, a)
}
