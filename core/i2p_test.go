package core

import (
	"encoding/json"
	"github.com/metacubex/mihomo/config"
	"os"
	"reflect"
	"testing"
)

func TestI2PPolicyOnlyRoutesInternalDomains(t *testing.T) {
	for _, platform := range []string{"linux", "android"} {
		settings := defaultSettings()
		obj(settings["direct"])["domains"] = []any{"example.com"}
		cfg, err := makeP2PConfig("i2p", Object{"type": "socks5", "server": "127.0.0.1", "port": 12345, "udp": false}, settings, platform, 123)
		if err != nil {
			t.Fatal(err)
		}
		if !reflect.DeepEqual(cfg["rules"], []string{"NETWORK,udp,REJECT", "DOMAIN-SUFFIX,i2p,VPN", "MATCH,REJECT"}) {
			t.Fatal("I2P permits an external route")
		}
		for _, key := range []string{"nameserver", "proxy-server-nameserver", "direct-nameserver"} {
			if !reflect.DeepEqual(obj(cfg["dns"])[key], []string{"rcode://success"}) {
				t.Fatalf("external DNS: %s", key)
			}
		}
		if !reflect.DeepEqual(obj(cfg["dns"])["default-nameserver"], []string{"127.0.0.1:1"}) {
			t.Fatal("I2P bootstrap DNS is not confined to loopback")
		}
		if obj(cfg["sniffer"])["enable"] != false || cfg["mixed-port"] != 0 {
			t.Fatal("I2P exposes an unintended proxy or sniffer")
		}
		obj(cfg["tun"])["enable"] = false
		delete(obj(cfg["tun"]), "file-descriptor")
		b, _ := json.Marshal(cfg)
		if _, err = config.Parse(b); err != nil {
			t.Fatalf("%s I2P config rejected: %v", platform, err)
		}
	}
}

func TestI2PFailureRetainsBlockingTunnelAndRejectsSwitch(t *testing.T) {
	dir := t.TempDir()
	os.Chmod(dir, 0700)
	engine := &fakeEngine{running: true}
	c, err := New(dir, "linux", engine)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	c.network = "i2p"
	c.tor = &torSession{}
	for _, r := range []Object{{"action": "Connect", "network": "tor"}, {"action": "Connect", "network": "vps"}, {"action": "ApplySettings", "settings": defaultSettings()}, {"action": "Autoconnect", "enabled": true}} {
		if response := request(t, c, r); response["ok"] != false {
			t.Fatal("I2P allowed an active network policy switch")
		}
	}
	s := c.snapshot()
	if s["network"] != "i2p" || s["running"] != false || s["phase"] != "recovery" || !engine.running {
		t.Fatal("I2P failure dropped the blocking tunnel")
	}
	c.tor = nil
	if response := request(t, c, Object{"action": "Disconnect"}); response["ok"] != true || engine.running {
		t.Fatal("explicit I2P disconnect failed")
	}
}
