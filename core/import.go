package core

import (
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/netip"
	"net/url"
	"regexp"
	"strconv"
	"strings"
	"unicode/utf8"

	"github.com/metacubex/mihomo/adapter"
	"github.com/metacubex/mihomo/common/convert"
	"gopkg.in/yaml.v3"
)

// YAML's syntax tree is checked before decoding: duplicates, aliases and deep
// structures must not silently change the meaning of a credential document.
func safeDocument(text string) (any, error) {
	if (strings.HasPrefix(text, "{") || strings.HasPrefix(text, "[")) && !json.Valid([]byte(text)) {
		return nil, errors.New("Invalid or trailing profile JSON")
	}
	var doc yaml.Node
	decoder := yaml.NewDecoder(strings.NewReader(text))
	if err := decoder.Decode(&doc); err != nil {
		return nil, errors.New("Invalid profile JSON/YAML")
	}
	var extra yaml.Node
	if decoder.Decode(&extra) != io.EOF {
		return nil, errors.New("Multiple or trailing YAML documents are not allowed")
	}
	count := 0
	var walk func(*yaml.Node, int) error
	walk = func(n *yaml.Node, depth int) error {
		count++
		if depth > 12 || count > 20000 || n.Kind == yaml.AliasNode || n.Anchor != "" {
			return errors.New("Profile structure exceeds limits or contains aliases")
		}
		if n.Kind == yaml.MappingNode {
			seen := map[string]bool{}
			for i := 0; i < len(n.Content); i += 2 {
				k := n.Content[i]
				key := strings.ToLower(k.Value)
				if k.Kind != yaml.ScalarNode || len(key) > 80 || seen[key] || key == "<<" {
					return errors.New("Duplicate or invalid profile field")
				}
				seen[key] = true
			}
		}
		for _, c := range n.Content {
			if err := walk(c, depth+1); err != nil {
				return err
			}
		}
		return nil
	}
	if err := walk(&doc, 0); err != nil {
		return nil, err
	}
	var value any
	if err := doc.Decode(&value); err != nil {
		return nil, errors.New("Invalid profile document")
	}
	return value, nil
}

func Import(text, name string) ([]Object, error) {
	return importDepth(text, name, 0)
}
func importDepth(text, name string, depth int) ([]Object, error) {
	text = strings.TrimSpace(strings.TrimPrefix(text, "\ufeff"))
	if len(text) == 0 || len(text) > MaxImport || !utf8.ValidString(text) || strings.ContainsRune(text, 0) {
		return nil, errors.New("Profile input must be UTF-8 and at most 512 KiB")
	}
	var nodes []Object
	if strings.HasPrefix(text, "[Interface]") {
		n, err := wireguard(text)
		if err != nil {
			return nil, err
		}
		nodes = []Object{n}
	} else if strings.HasPrefix(text, "client\n") || strings.HasPrefix(text, "client\r\n") || strings.Contains(text, "\nremote ") {
		n, err := openVPN(text)
		if err != nil {
			return nil, err
		}
		nodes = []Object{n}
	} else if regexp.MustCompile(`^[a-zA-Z][a-zA-Z0-9+.-]*://`).MatchString(text) {
		// Parse each line separately; the upstream subscription converter otherwise
		// silently skips malformed entries and would cause a partial import.
		for _, line := range strings.Split(text, "\n") {
			line = strings.TrimSpace(line)
			if line == "" {
				continue
			}
			if strings.HasPrefix(line, "mycelium://") {
				n, err := myceliaLink(line)
				if err != nil {
					return nil, err
				}
				nodes = append(nodes, n)
				continue
			}
			n, err := convert.ConvertsV2Ray([]byte(line))
			if err != nil || len(n) != 1 {
				return nil, errors.New("Invalid or unsupported server link")
			}
			nodes = append(nodes, n[0])
		}
	} else {
		if depth < 2 {
			for _, encoding := range []*base64.Encoding{base64.StdEncoding, base64.RawStdEncoding, base64.URLEncoding, base64.RawURLEncoding} {
				decoded, err := encoding.DecodeString(strings.Join(strings.Fields(text), ""))
				if err == nil && utf8.Valid(decoded) && (strings.Contains(string(decoded), "://") || strings.HasPrefix(string(decoded), "{")) {
					return importDepth(string(decoded), name, depth+1)
				}
			}
		}
		value, err := safeDocument(text)
		if err != nil {
			return nil, err
		}
		switch v := value.(type) {
		case map[string]any:
			if p, ok := v["proxy"]; ok && len(v) == 2 && v["schema"] == 1 {
				nodes = []Object{obj(p)}
			} else if p, ok := v["proxies"]; ok {
				if len(v) != 1 {
					return nil, errors.New("Import server nodes only; global configuration is not accepted")
				}
				for _, n := range array(p) {
					nodes = append(nodes, obj(n))
				}
			} else {
				nodes = []Object{v}
			}
		case []any:
			for _, n := range v {
				nodes = append(nodes, obj(n))
			}
		default:
			return nil, errors.New("Expected a server node or list")
		}
	}
	if len(nodes) == 0 || len(nodes) > 128 {
		return nil, errors.New("Import must contain 1 to 128 servers")
	}
	for i, n := range nodes {
		if len(nodes) == 1 && strings.TrimSpace(name) != "" {
			n["name"] = strings.TrimSpace(name)
		}
		if str(n["name"]) == "" {
			n["name"] = fmt.Sprintf("Server %d", i+1)
		}
		if err := validateNode(n); err != nil {
			return nil, err
		}
	}
	return nodes, nil
}

func myceliaLink(text string) (Object, error) {
	fail := func() (Object, error) { return nil, errors.New("Invalid Mycelia server link") }
	u, err := url.Parse(text)
	if err != nil || u.User == nil || u.Path != "" || u.Opaque != "" {
		return fail()
	}
	if _, password := u.User.Password(); password {
		return fail()
	}
	q, err := url.ParseQuery(u.RawQuery)
	if err != nil {
		return fail()
	}
	n := Object{"type": "mycelium", "server": u.Hostname(), "port": u.Port(), "seed": u.User.Username(), "name": u.Fragment, "udp": true}
	for k, v := range q {
		if len(v) != 1 || v[0] == "" {
			return fail()
		}
		switch k {
		case "sni", "client-fingerprint", "transport", "udp-transport":
			n[k] = v[0]
		case "fp":
			n["fingerprint"] = v[0]
		case "udp":
			b, e := strconv.ParseBool(v[0])
			if e != nil {
				return fail()
			}
			n[k] = b
		default:
			return fail()
		}
	}
	return n, nil
}

func validateNode(n Object) error {
	typ := str(n["type"])
	fields, ok := nodeFields[typ]
	if !ok {
		return errors.New("Unsupported profile protocol")
	}
	allowed := " name type server port tfo mptcp ip-version " + fields + " "
	for k := range n {
		if !strings.Contains(allowed, " "+k+" ") {
			return fmt.Errorf("Unsupported profile field: %s", k)
		}
	}
	if !validName(str(n["name"])) {
		return errors.New("Server name must contain 1 to 80 printable characters")
	}
	host := str(n["server"])
	if host == "" || len(host) > 253 || strings.ContainsAny(host, "\r\n\x00 /\\,@") {
		return errors.New("Invalid server address")
	}
	port, err := strconv.Atoi(fmt.Sprint(n["port"]))
	if err != nil || port < 1 || port > 65535 {
		return errors.New("Invalid server port")
	}
	n["port"] = port
	var inspect func(any, string, int) error
	inspect = func(v any, key string, depth int) error {
		if depth > 10 {
			return errors.New("Profile nesting exceeds limit")
		}
		switch x := v.(type) {
		case map[string]any:
			for k, y := range x {
				if strings.HasSuffix(k, "-file") || k == "file" || k == "command" || k == "script" || k == "dialer-proxy" {
					return errors.New("Profile cannot reference files, commands or other proxies")
				}
				if err := inspect(y, k, depth+1); err != nil {
					return err
				}
			}
		case []any:
			if len(x) > 128 {
				return errors.New("Profile array exceeds limit")
			}
			for _, y := range x {
				if err := inspect(y, key, depth+1); err != nil {
					return err
				}
			}
		case string:
			if len(x) > 65536 || strings.ContainsRune(x, 0) {
				return errors.New("Invalid profile option")
			}
		}
		return nil
	}
	if err := inspect(n, "", 0); err != nil {
		return err
	}
	for _, key := range []string{"certificate", "ca", "cert", "key", "tls-auth", "tls-crypt", "tls-crypt-v2"} {
		if s := str(n[key]); s != "" && !strings.Contains(s, "-----BEGIN ") {
			return errors.New("Certificates and OpenVPN keys must be inline PEM")
		}
	}
	if key := str(n["private-key"]); key != "" {
		if typ == "wireguard" || typ == "masque" {
			b, e := base64.StdEncoding.DecodeString(key)
			if e != nil || len(b) != 32 {
				return errors.New("Invalid private key")
			}
		} else if !strings.Contains(key, "-----BEGIN ") {
			return errors.New("Private keys must be inline PEM")
		}
	}
	// The upstream parser validates protocol-specific requirements; never return
	// its error because some adapters include credentials in diagnostics.
	proxy, err := adapter.ParseProxy(n)
	if err != nil {
		return fmt.Errorf("Invalid %s server options", typ)
	}
	_ = proxy.Close()
	return nil
}

func wireguard(text string) (Object, error) {
	sections := map[string]map[string]string{}
	section := ""
	for _, line := range strings.Split(text, "\n") {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "#") || strings.HasPrefix(line, ";") {
			continue
		}
		if strings.HasPrefix(line, "[") {
			if line != "[Interface]" && line != "[Peer]" {
				return nil, errors.New("Unsupported WireGuard section")
			}
			section = line
			if sections[section] != nil {
				return nil, errors.New("Exactly one WireGuard peer is supported")
			}
			sections[section] = map[string]string{}
			continue
		}
		k, v, ok := strings.Cut(line, "=")
		k = strings.ToLower(strings.TrimSpace(k))
		if !ok || section == "" || sections[section][k] != "" {
			return nil, errors.New("Invalid or duplicate WireGuard field")
		}
		sections[section][k] = strings.TrimSpace(v)
	}
	i, p := sections["[Interface]"], sections["[Peer]"]
	host, port, err := net.SplitHostPort(p["endpoint"])
	if err != nil {
		return nil, errors.New("Invalid WireGuard endpoint")
	}
	n := Object{"name": "WireGuard / AmneziaWG", "type": "wireguard", "server": host, "port": port, "private-key": i["privatekey"], "public-key": p["publickey"], "udp": true, "mtu": 1280}
	for _, a := range strings.Split(i["address"], ",") {
		a = strings.TrimSpace(strings.Split(a, "/")[0])
		ip, e := netip.ParseAddr(a)
		if e != nil {
			return nil, errors.New("Invalid WireGuard address")
		}
		if ip.Is4() {
			n["ip"] = a
		} else {
			n["ipv6"] = a
		}
	}
	for k, v := range p {
		switch k {
		case "endpoint", "publickey", "allowedips":
		case "presharedkey":
			n["pre-shared-key"] = v
		case "persistentkeepalive":
			num, e := strconv.Atoi(v)
			if e != nil {
				return nil, e
			}
			n["persistent-keepalive"] = num
		default:
			return nil, errors.New("Unsupported WireGuard peer option")
		}
	}
	awg := Object{}
	for k, v := range i {
		switch k {
		case "privatekey", "address", "dns", "listenport":
		case "mtu":
			num, e := strconv.Atoi(v)
			if e != nil || num < 576 || num > 9000 {
				return nil, errors.New("Invalid MTU")
			}
			n["mtu"] = num
		default:
			if !strings.Contains(" jc jmin jmax s1 s2 s3 s4 h1 h2 h3 h4 i1 i2 i3 i4 i5 j1 j2 j3 itime ", " "+k+" ") {
				return nil, errors.New("Unsupported WireGuard interface option")
			}
			if strings.Contains(" jc jmin jmax s1 s2 s3 s4 itime ", " "+k+" ") {
				num, e := strconv.Atoi(v)
				if e != nil || num < 0 {
					return nil, errors.New("Invalid AmneziaWG number")
				}
				awg[k] = num
			} else {
				awg[k] = v
			}
		}
	}
	if len(awg) > 0 {
		awg["version"] = 2
		if awg["j1"] != nil || awg["itime"] != nil {
			awg["version"] = 1
		}
		n["amnezia-wg-option"] = awg
	}
	return n, nil
}
