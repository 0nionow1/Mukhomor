package core

import (
	"errors"
	"strconv"
	"strings"
)

func openVPN(text string) (Object, error) {
	n := Object{"name": "OpenVPN", "type": "openvpn", "dev": "tun", "udp": true}
	lines := strings.Split(text, "\n")
	seen := map[string]bool{}
	authRequired := false
	bad := func() (Object, error) {
		return nil, errors.New("Invalid OpenVPN directive, duplicate option or external file reference")
	}
	for i := 0; i < len(lines); i++ {
		line := strings.TrimSpace(lines[i])
		if line == "" || strings.HasPrefix(line, "#") || strings.HasPrefix(line, ";") {
			continue
		}
		if strings.HasPrefix(line, "<") {
			key := strings.TrimSuffix(strings.TrimPrefix(line, "<"), ">")
			if !strings.Contains(" ca cert key tls-auth tls-crypt tls-crypt-v2 auth-user-pass ", " "+key+" ") || seen["inline-"+key] {
				return bad()
			}
			seen["inline-"+key] = true
			block := []string{}
			closed := false
			for i++; i < len(lines); i++ {
				if strings.TrimSpace(lines[i]) == "</"+key+">" {
					closed = true
					break
				}
				block = append(block, strings.TrimSuffix(lines[i], "\r"))
			}
			if !closed {
				return bad()
			}
			if key == "auth-user-pass" {
				if len(block) != 2 {
					return bad()
				}
				n["username"] = block[0]
				n["password"] = block[1]
			} else {
				n[key] = strings.Join(block, "\n") + "\n"
			}
			continue
		}
		p := strings.Fields(line)
		key := p[0]
		if seen[key] {
			return bad()
		}
		seen[key] = true
		switch key {
		case "remote":
			if len(p) < 3 || len(p) > 4 {
				return bad()
			}
			n["server"] = p[1]
			n["port"] = p[2]
			if len(p) == 4 {
				n["proto"] = p[3]
			}
		case "auth-user-pass":
			if len(p) != 1 {
				return bad()
			}
			authRequired = true
		case "client", "tls-client", "nobind", "persist-key", "persist-tun":
			if len(p) != 1 {
				return bad()
			}
		case "remote-cert-tls":
			if len(p) != 2 || p[1] != "server" {
				return bad()
			}
		case "verb":
			if len(p) != 2 || len(p[1]) != 1 || p[1][0] < '0' || p[1][0] > '9' {
				return bad()
			}
		case "proto", "dev", "cipher", "data-ciphers", "data-ciphers-fallback", "auth", "comp-lzo", "key-direction", "ping", "ping-restart", "tran-window", "handshake-timeout", "tun-mtu":
			if len(p) != 2 {
				return bad()
			}
			target := key
			if key == "tun-mtu" {
				target = "mtu"
			}
			if key == "data-ciphers" {
				n[target] = strings.Split(p[1], ":")
			} else if strings.Contains(" ping ping-restart tran-window handshake-timeout tun-mtu ", " "+key+" ") {
				v, e := strconv.Atoi(p[1])
				if e != nil || v < 0 || v > 99999999 {
					return bad()
				}
				n[target] = v
			} else {
				n[target] = p[1]
			}
		default:
			return bad()
		}
	}
	if n["server"] == nil || authRequired && n["username"] == nil {
		return bad()
	}
	return n, nil
}
