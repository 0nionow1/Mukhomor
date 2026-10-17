package main

import (
	"encoding/base64"
	"fmt"
	"io"
	"net"
	"strings"
	"testing"
	"time"
)

func TestI2PDestinationScope(t *testing.T) {
	for _, host := range []string{"site.i2p", strings.Repeat("a", 52) + ".b32.i2p", "a.b.i2p"} {
		if !validI2PHost(host) {
			t.Fatalf("valid destination refused: %q", host)
		}
	}
	for _, host := range []string{"example.com", "i2p", ".i2p", "site.i2p.example.com", "site.i2p.", "127.0.0.1", "::1", "a..i2p", "-a.i2p", "a-.i2p", "a/i2p", "a\x00.i2p", "a\n.i2p", "тест.i2p", strings.Repeat("a", 64) + ".i2p"} {
		if validI2PHost(host) {
			t.Fatalf("unsafe destination accepted: %q", host)
		}
	}
}

func TestI2PSocksRequiresAuthenticationAndDomainConnect(t *testing.T) {
	for _, tc := range []struct {
		name, password, host string
		command, addressType byte
		allow                bool
	}{
		{"internal", "secret", "site.i2p", 1, 3, true},
		{"wrong password", "invalid", "site.i2p", 1, 3, false},
		{"clearnet", "secret", "example.com", 1, 3, false},
		{"UDP", "secret", "site.i2p", 3, 3, false},
		{"BIND", "secret", "site.i2p", 2, 3, false},
		{"IPv4", "secret", "site.i2p", 1, 1, false},
		{"IPv6", "secret", "site.i2p", 1, 4, false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			server, client := net.Pipe()
			defer client.Close()
			result := make(chan error, 1)
			go func() {
				defer server.Close()
				server.SetDeadline(time.Now().Add(2 * time.Second))
				_, _, err := i2pRequest(server, "user", "secret")
				result <- err
			}()
			client.SetDeadline(time.Now().Add(2 * time.Second))
			client.Write([]byte{5, 1, 2})
			reply := make([]byte, 2)
			io.ReadFull(client, reply)
			if reply[0] != 5 || reply[1] != 2 {
				t.Fatal("authentication not required")
			}
			auth := append([]byte{1, 4}, []byte("user")...)
			auth = append(auth, byte(len(tc.password)))
			auth = append(auth, []byte(tc.password)...)
			client.Write(auth)
			io.ReadFull(client, reply)
			if reply[1] == 0 {
				request := append([]byte{5, tc.command, 0, tc.addressType, byte(len(tc.host))}, []byte(tc.host)...)
				client.Write(append(request, 0, 80))
			}
			if err := <-result; (err == nil) != tc.allow {
				t.Fatalf("unexpected scope result: %v", err)
			}
		})
	}
	server, client := net.Pipe()
	defer client.Close()
	done := make(chan error, 1)
	go func() { defer server.Close(); _, _, e := i2pRequest(server, "user", "secret"); done <- e }()
	client.Write([]byte{5, 1, 0})
	reply := make([]byte, 2)
	io.ReadFull(client, reply)
	if reply[1] != 255 || <-done == nil {
		t.Fatal("unauthenticated SOCKS accepted")
	}
}

func TestI2PUnderlayRefusesLocalDNSAndUnauthenticatedRequests(t *testing.T) {
	// A nil underlay deliberately panics if any invalid request reaches Dial.
	token := strings.Repeat("a", 64)
	ln, err := startI2PUnderlay(nil, token)
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()
	auth := "Basic " + base64.StdEncoding.EncodeToString([]byte(token+":"+token))
	for _, tc := range []struct{ destination, auth string }{
		{"127.0.0.1:80", auth}, {"192.168.1.1:443", auth}, {"[::1]:443", auth},
		{"169.254.169.254:80", auth}, {"1.1.1.1:53", auth}, {"1.1.1.1:853", auth},
		{"example.com:443", auth}, {"1.1.1.1:443", "Basic invalid"},
	} {
		c, e := net.DialTimeout("tcp", ln.Addr().String(), time.Second)
		if e != nil {
			t.Fatal(e)
		}
		c.SetDeadline(time.Now().Add(2 * time.Second))
		fmt.Fprintf(c, "CONNECT %s HTTP/1.1\r\nHost: %s\r\nProxy-Authorization: %s\r\n\r\n", tc.destination, tc.destination, tc.auth)
		b := make([]byte, 1)
		n, e := c.Read(b)
		c.Close()
		if n != 0 || e == nil {
			t.Fatal("unsafe underlay request accepted")
		}
	}
}
