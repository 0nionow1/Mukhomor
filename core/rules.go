package core

import (
	"encoding/json"
	"errors"
	"fmt"
	"net/netip"
	"regexp"
	"strings"

	assets "github.com/0nionow1/Mukhomor"
)

var sections = []string{"process_names", "process_paths", "domains", "domain_suffixes", "ip_cidrs", "rule_set_exclusions"}
var domainPattern = regexp.MustCompile(`^[a-zA-Z0-9_](?:[a-zA-Z0-9_.-]*[a-zA-Z0-9_])?$`)
var packagePattern = regexp.MustCompile(`^[a-zA-Z][a-zA-Z0-9_]*(?:\.[a-zA-Z][a-zA-Z0-9_]*)+$`)

func defaultSettings() Object {
	b, _ := assets.Files.ReadFile("assets/settings.default.json")
	var s Object
	_ = json.Unmarshal(b, &s)
	return s
}
func contains(v any, s string) bool {
	for _, x := range array(v) {
		if str(x) == s {
			return true
		}
	}
	return false
}

func validateSettings(s Object) ([]any, error) {
	b, err := json.Marshal(s)
	if err != nil || len(b) > MaxSettings {
		return nil, errors.New("Settings exceed 2 MiB; reduce rule list size. Your draft has been kept.")
	}
	if s["schema"] != float64(2) && s["schema"] != 2 {
		return nil, errors.New("Unsupported settings schema")
	}
	mode := str(obj(s["routing"])["mode"])
	if mode != "split" && mode != "full" {
		return nil, errors.New("Invalid routing mode")
	}
	if _, ok := obj(s["routing"])["allow_lan"].(bool); !ok {
		return nil, errors.New("Invalid LAN preference")
	}
	owners := []Object{{"id": "direct", "rules": s["direct"], "domain_rule_sets": obj(s["direct"])["domain_rule_sets"]}}
	if len(array(s["presets"])) > 128 {
		return nil, errors.New("Too many rule presets")
	}
	ids := map[string]bool{}
	for _, p := range array(s["presets"]) {
		o := obj(p)
		id := str(o["id"])
		if id == "" || ids[id] || !validName(str(o["name"])) {
			return nil, errors.New("Invalid or duplicate preset")
		}
		ids[id] = true
		owners = append(owners, o)
	}
	issues := []any{}
	for _, o := range owners {
		for _, key := range sections {
			v, ok := obj(o["rules"])[key].([]any)
			if !ok {
				return nil, fmt.Errorf("Invalid rule list: %s", key)
			}
			for i, item := range v {
				s, ok := item.(string)
				valid := ok && len(s) > 0 && len(s) <= 4096 && !strings.ContainsAny(s, "\x00\r\n,")
				if valid {
					switch key {
					case "domains", "domain_suffixes", "rule_set_exclusions":
						valid = len(s) <= 253 && domainPattern.MatchString(s) && !strings.Contains(s, "..")
					case "ip_cidrs":
						_, e := netip.ParsePrefix(s)
						valid = e == nil
					case "process_paths":
						valid = strings.HasPrefix(s, "/") || strings.Contains(s, ":\\")
					}
				}
				if !valid && len(issues) < 100 {
					issues = append(issues, Object{"preset_id": o["id"], "section": key, "line": i + 1, "value": item, "message": "Invalid rule"})
				}
			}
		}
		for _, source := range array(o["domain_rule_sets"]) {
			if !strings.Contains(" russia steam ozon games ", " "+str(source)+" ") {
				return nil, errors.New("Unknown domain rule source")
			}
		}
	}
	return issues, nil
}

func effective(s Object) Object {
	d := obj(clone(s["direct"]))
	for _, key := range append(append([]string{}, sections...), "domain_rule_sets") {
		if d[key] == nil {
			d[key] = []any{}
		}
	}
	for _, p := range array(s["presets"]) {
		o := obj(p)
		if o["enabled"] != true {
			continue
		}
		for _, key := range sections {
			if !contains(o["disabled_sections"], key) {
				d[key] = append(array(d[key]), array(obj(o["rules"])[key])...)
			}
		}
		d["domain_rule_sets"] = append(array(d["domain_rule_sets"]), array(o["domain_rule_sets"])...)
	}
	for k, v := range d {
		seen := map[string]bool{}
		values := []any{}
		for _, x := range array(v) {
			s := str(x)
			if !seen[s] {
				values = append(values, s)
				seen[s] = true
			}
		}
		d[k] = values
	}
	return d
}

func androidPackages(s Object) []string {
	if str(obj(s["routing"])["mode"]) == "full" {
		return nil
	}
	var result []string
	for _, v := range array(effective(s)["process_names"]) {
		if packagePattern.MatchString(str(v)) && !strings.HasSuffix(str(v), ".exe") {
			result = append(result, str(v))
		}
	}
	return result
}

func makeConfig(node, s Object, platform string, fd int, lists Object) (Object, error) {
	issues, err := validateSettings(s)
	if err != nil {
		return nil, err
	}
	if len(issues) > 0 {
		return nil, errors.New("Fix invalid rules before connecting")
	}
	n := obj(clone(node))
	n["name"] = "VPN"
	rules := []string{}
	full := str(obj(s["routing"])["mode"]) == "full"
	d := effective(s)
	add := func(kind, key string) {
		for _, v := range array(d[key]) {
			rules = append(rules, kind+","+str(v)+",DIRECT")
		}
	}
	if !full {
		if platform == "linux" {
			add("PROCESS-NAME", "process_names")
			add("PROCESS-PATH", "process_paths")
		}
		add("DOMAIN", "domains")
		add("DOMAIN-SUFFIX", "domain_suffixes")
		for _, v := range array(d["rule_set_exclusions"]) {
			rules = append(rules, "DOMAIN-SUFFIX,"+str(v)+",VPN")
		}
		for _, source := range array(d["domain_rule_sets"]) {
			b, e := assets.Files.ReadFile("lists/seed-" + str(source) + ".list")
			if e != nil {
				return nil, errors.New("Bundled rule source missing")
			}
			if saved := str(lists[str(source)]); saved != "" {
				b = []byte(saved)
			}
			for _, line := range strings.Split(string(b), "\n") {
				line = strings.TrimSpace(line)
				if line == "" || strings.HasPrefix(line, "#") {
					continue
				}
				kind := "DOMAIN"
				if strings.HasPrefix(line, "+.") {
					kind = "DOMAIN-SUFFIX"
					line = strings.TrimPrefix(line, "+.")
				}
				rules = append(rules, kind+","+line+",DIRECT")
			}
		}
		for _, v := range array(d["ip_cidrs"]) {
			kind := "IP-CIDR"
			if strings.Contains(str(v), ":") {
				kind = "IP-CIDR6"
			}
			rules = append(rules, kind+","+str(v)+",DIRECT")
		}
	}
	if !full || obj(s["routing"])["allow_lan"] == true {
		for _, cidr := range []string{"127.0.0.0/8", "10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16", "::1/128", "fc00::/7", "fe80::/10"} {
			kind := "IP-CIDR"
			if strings.Contains(cidr, ":") {
				kind = "IP-CIDR6"
			}
			rules = append(rules, kind+","+cidr+",DIRECT,no-resolve")
		}
	}
	rules = append(rules, "MATCH,VPN")
	tun := Object{"enable": true, "device": "mukhomor0", "stack": "mixed", "auto-route": platform == "linux", "auto-detect-interface": platform == "linux", "strict-route": true, "dns-hijack": []string{"any:53", "tcp://any:53"}, "mtu": 1500, "inet4-address": []string{"172.19.0.1/30"}, "inet6-address": []string{"fdfe:dcba:9876::1/126"}, "iproute2-table-index": 20880, "iproute2-rule-index": 9080}
	if platform == "android" {
		if fd < 0 {
			return nil, errors.New("Android VPN permission is required")
		}
		tun["file-descriptor"] = fd
		// Android excludes this app's UID from its VPN. A kernel TCP stack
		// would send its accepted socket replies over that excluded route.
		// Keep both TCP and UDP inside the userspace stack instead.
		tun["stack"] = "gvisor"
		tun["auto-route"] = false
		tun["auto-detect-interface"] = false
	}
	dns := Object{"enable": true, "ipv6": true, "enhanced-mode": "fake-ip", "fake-ip-range": "198.18.0.1/16", "fake-ip-filter": []string{"*.lan", "*.local", "localhost"}, "nameserver": []string{"https://1.1.1.1/dns-query#VPN", "https://8.8.8.8/dns-query#VPN"}, "default-nameserver": []string{"1.1.1.1", "8.8.8.8"}, "proxy-server-nameserver": []string{"https://1.1.1.1/dns-query#DIRECT"}, "direct-nameserver": []string{"https://1.1.1.1/dns-query#DIRECT"}, "respect-rules": true}
	mode := "off"
	if platform == "linux" && !full {
		mode = "strict"
	}
	return Object{"mode": "rule", "log-level": "silent", "ipv6": true, "allow-lan": false, "mixed-port": 0, "external-controller": "", "find-process-mode": mode, "geo-auto-update": false, "profile": Object{"store-fake-ip": false}, "proxies": []Object{n}, "rules": rules, "dns": dns, "tun": tun}, nil
}
