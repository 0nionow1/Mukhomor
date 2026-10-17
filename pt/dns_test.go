package main

import (
	"context"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"sync/atomic"
	"testing"
	"time"

	"github.com/miekg/dns"
)

func dnsFixture(t *testing.T, alter func(*dns.Msg), calls *atomic.Int32) *httptest.Server {
	t.Helper()
	server := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		calls.Add(1)
		if r.Method != "POST" || r.Header.Get("Content-Type") != "application/dns-message" {
			t.Error("unexpected DNS request")
		}
		wire, _ := io.ReadAll(r.Body)
		query := new(dns.Msg)
		if query.Unpack(wire) != nil {
			t.Error("invalid DNS query")
			return
		}
		answer := new(dns.Msg)
		answer.SetReply(query)
		answer.Answer = []dns.RR{&dns.A{Hdr: dns.RR_Header{Name: query.Question[0].Name, Rrtype: dns.TypeA, Class: dns.ClassINET}, A: net.ParseIP("93.184.216.34")}}
		if alter != nil {
			alter(answer)
		}
		wire, _ = answer.Pack()
		w.Header().Set("Content-Type", "application/dns-message")
		w.Write(wire)
	}))
	t.Cleanup(server.Close)
	return server
}

func TestEncryptedDNSFailoverAndPreference(t *testing.T) {
	var failed, healthy atomic.Int32
	first := dnsFixture(t, func(m *dns.Msg) { m.Rcode = dns.RcodeServerFailure }, &failed)
	second := dnsFixture(t, nil, &healthy)
	n := &underlay{dns: []dohResolver{{first.URL, first.Client()}, {second.URL, second.Client()}}}
	for range 2 {
		ip, err := n.resolve(context.Background(), "bridge.example")
		if err != nil || !ip.Equal(net.ParseIP("93.184.216.34")) {
			t.Fatal("encrypted DNS fallback failed", err)
		}
	}
	if failed.Load() != 1 || healthy.Load() != 2 {
		t.Fatal("failed resolver was repeatedly retried")
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, err := n.resolve(ctx, "bridge.example"); err == nil || healthy.Load() != 2 {
		t.Fatal("cancelled lookup issued a request")
	}
}

func TestEncryptedDNSRejectsUntrustedOrUnrelatedAnswers(t *testing.T) {
	cases := map[string]func(*dns.Msg){
		"wrong transaction": func(m *dns.Msg) { m.Id++ },
		"wrong question":    func(m *dns.Msg) { m.Question[0].Name = "other.example." },
		"unrelated address": func(m *dns.Msg) { m.Answer[0].Header().Name = "other.example." },
		"private address":   func(m *dns.Msg) { m.Answer[0].(*dns.A).A = net.ParseIP("127.0.0.1") },
		"truncated":         func(m *dns.Msg) { m.Truncated = true },
	}
	for name, alter := range cases {
		t.Run(name, func(t *testing.T) {
			var count atomic.Int32
			server := dnsFixture(t, alter, &count)
			n := &underlay{dns: []dohResolver{{server.URL, server.Client()}}}
			if _, err := n.resolve(context.Background(), "bridge.example"); err == nil {
				t.Fatal("accepted unsafe DNS response")
			}
		})
	}
	var count atomic.Int32
	server := dnsFixture(t, nil, &count)
	// Its certificate is not trusted by the regular client; no insecure retry.
	n := &underlay{dns: []dohResolver{{server.URL, &http.Client{Timeout: time.Second}}}}
	if _, err := n.resolve(context.Background(), "bridge.example"); err == nil {
		t.Fatal("accepted untrusted TLS certificate")
	}
}
