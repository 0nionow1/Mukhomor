//go:build linux && !android

package core

import (
	"errors"
	"fmt"
	"os"
	"reflect"
	"strings"

	"github.com/metacubex/nftables"
	"github.com/metacubex/nftables/binaryutil"
	"github.com/metacubex/nftables/expr"
	"golang.org/x/sys/unix"
)

// Only the bundled Arti helper sets this mark, on its physical TCP sockets.
// Setting SO_MARK requires CAP_NET_ADMIN, or CAP_NET_RAW on Linux >= 5.17.
// Ordinary applications cannot create this exception. There is deliberately no
// established-connection exception: pre-existing physical streams also stop.
const torSocketMark = 0x4d544f52

// Separate privileged mark, used only by the bundled bridge adapter for WebRTC.
const torBridgeUDPMark = 0x4d505455

type linuxTorGuard struct{ table *nftables.Table }

func newTorGuard() torGuard {
	return &linuxTorGuard{table: &nftables.Table{Name: fmt.Sprintf("mukhomor_tor_%d", os.Getuid()), Family: nftables.TableFamilyINet}}
}

// Used by the privileged uninstaller before removing the recovery executable.
func TorGuardsPresent() (bool, error) {
	tables, err := (&nftables.Conn{}).ListTablesOfFamily(nftables.TableFamilyINet)
	if err != nil {
		return false, err
	}
	for _, table := range tables {
		if strings.HasPrefix(table.Name, "mukhomor_tor_") {
			return true, nil
		}
	}
	return false, nil
}

func (g *linuxTorGuard) present() (bool, error) {
	tables, err := (&nftables.Conn{}).ListTablesOfFamily(nftables.TableFamilyINet)
	if err != nil {
		return false, errors.New("Cannot inspect Tor traffic protection; check Linux nftables and installation privileges")
	}
	for _, table := range tables {
		if table.Name == g.table.Name {
			return true, nil
		}
	}
	return false, nil
}

func (g *linuxTorGuard) chain() *nftables.Chain {
	drop := nftables.ChainPolicyDrop
	return &nftables.Chain{Name: "output", Table: g.table, Type: nftables.ChainTypeFilter,
		Hooknum: nftables.ChainHookOutput, Priority: nftables.ChainPriorityFilter, Policy: &drop}
}

func torGuardRules() [][]expr.Any {
	meta := func(key expr.MetaKey, value []byte) []expr.Any {
		return []expr.Any{&expr.Meta{Key: key, Register: 1}, &expr.Cmp{Op: expr.CmpOpEq, Register: 1, Data: value}}
	}
	accept := func(match []expr.Any) []expr.Any { return append(match, &expr.Verdict{Kind: expr.VerdictAccept}) }
	name := func(s string) []byte { b := make([]byte, 16); copy(b, s); return b }
	rules := [][]expr.Any{
		accept(meta(expr.MetaKeyOIFNAME, name("lo"))),
		accept(meta(expr.MetaKeyOIFNAME, name("mukhomor0"))),
	}
	// Neither plaintext DNS nor DoT is an underlay exception, even if marked.
	for _, transport := range []struct {
		protocol byte
		mark     uint32
	}{{unix.IPPROTO_TCP, torSocketMark}, {unix.IPPROTO_UDP, torBridgeUDPMark}} {
		tcp := meta(expr.MetaKeyL4PROTO, []byte{transport.protocol})
		tcp = append(tcp, meta(expr.MetaKeyMARK, binaryutil.NativeEndian.PutUint32(transport.mark))...)
		tcp = append(tcp, &expr.Payload{DestRegister: 1, Base: expr.PayloadBaseTransportHeader, Offset: 2, Len: 2},
			&expr.Cmp{Op: expr.CmpOpNeq, Register: 1, Data: binaryutil.BigEndian.PutUint16(53)},
			&expr.Cmp{Op: expr.CmpOpNeq, Register: 1, Data: binaryutil.BigEndian.PutUint16(853)})
		rules = append(rules, accept(tcp))
	}
	// Minimal link maintenance: DHCPv4 broadcast, DHCPv6 link-local multicast,
	// and IPv6 neighbour/router discovery with the required hop limit of 255.
	for _, v6 := range []bool{false, true} {
		family, offset, destination, ports := byte(unix.NFPROTO_IPV4), uint32(16), []byte{255, 255, 255, 255}, []byte{0, 68, 0, 67}
		if v6 {
			family = unix.NFPROTO_IPV6
			offset = 24
			destination = []byte{255, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 2}
			ports = []byte{2, 34, 2, 35}
		}
		r := meta(expr.MetaKeyNFPROTO, []byte{family})
		r = append(r, meta(expr.MetaKeyL4PROTO, []byte{unix.IPPROTO_UDP})...)
		r = append(r, &expr.Payload{DestRegister: 1, Base: expr.PayloadBaseNetworkHeader, Offset: offset, Len: uint32(len(destination))}, &expr.Cmp{Op: expr.CmpOpEq, Register: 1, Data: destination},
			&expr.Payload{DestRegister: 1, Base: expr.PayloadBaseTransportHeader, Offset: 0, Len: 4}, &expr.Cmp{Op: expr.CmpOpEq, Register: 1, Data: ports})
		rules = append(rules, accept(r))
	}
	for _, kind := range []byte{133, 134, 135, 136} {
		r := meta(expr.MetaKeyNFPROTO, []byte{unix.NFPROTO_IPV6})
		r = append(r, meta(expr.MetaKeyL4PROTO, []byte{unix.IPPROTO_ICMPV6})...)
		r = append(r, &expr.Payload{DestRegister: 1, Base: expr.PayloadBaseNetworkHeader, Offset: 7, Len: 1}, &expr.Cmp{Op: expr.CmpOpEq, Register: 1, Data: []byte{255}},
			&expr.Payload{DestRegister: 1, Base: expr.PayloadBaseTransportHeader, Offset: 0, Len: 2}, &expr.Cmp{Op: expr.CmpOpEq, Register: 1, Data: []byte{kind, 0}})
		rules = append(rules, accept(r))
	}
	return rules
}

func (g *linuxTorGuard) verify() error {
	c := &nftables.Conn{}
	chains, err := c.ListChainsOfTableFamily(nftables.TableFamilyINet)
	if err != nil {
		return err
	}
	found := 0
	for _, chain := range chains {
		if chain.Table.Name != g.table.Name {
			continue
		}
		found++
		want := g.chain()
		if chain.Name != want.Name || chain.Type != want.Type || chain.Hooknum == nil || *chain.Hooknum != *want.Hooknum || chain.Priority == nil || *chain.Priority != *want.Priority || chain.Policy == nil || *chain.Policy != *want.Policy {
			return errors.New("Unexpected Tor firewall chain")
		}
		rules, err := c.GetRules(g.table, chain)
		if err != nil {
			return err
		}
		wantRules := torGuardRules()
		if len(rules) != len(wantRules) {
			return errors.New("Unexpected Tor firewall rule count")
		}
		for i, rule := range rules {
			if !reflect.DeepEqual(rule.Exprs, wantRules[i]) {
				return errors.New("Tor firewall readback mismatch")
			}
		}
	}
	if found != 1 {
		return errors.New("Tor firewall chain is missing")
	}
	return nil
}

func (g *linuxTorGuard) arm() error {
	present, err := g.present()
	if err != nil {
		return err
	}
	c := &nftables.Conn{}
	// Replacing an existing policy is one atomic netlink transaction: no gap.
	if present {
		c.DelTable(g.table)
	}
	c.AddTable(g.table)
	chain := c.AddChain(g.chain())
	for _, r := range torGuardRules() {
		c.AddRule(&nftables.Rule{Table: g.table, Chain: chain, Exprs: r})
	}
	if err = c.Flush(); err != nil {
		return errors.New("Cannot activate Tor traffic protection; nftables is required")
	}
	if err = g.verify(); err != nil {
		return errors.New("Cannot verify Tor traffic protection; disconnect to recover")
	}
	return nil
}

func (g *linuxTorGuard) disarm() error {
	present, err := g.present()
	if err != nil {
		return err
	}
	if !present {
		return nil
	}
	c := &nftables.Conn{}
	c.DelTable(g.table)
	if err = c.Flush(); err != nil {
		return errors.New("Cannot remove Tor traffic protection")
	}
	present, err = g.present()
	if err != nil || present {
		return errors.New("Cannot verify removal of Tor traffic protection")
	}
	return nil
}
