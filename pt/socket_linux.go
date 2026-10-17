//go:build linux && !android

package main

import (
	"errors"
	"golang.org/x/sys/unix"
	"strings"
	"syscall"
)

func (n *underlay) control(network, address string, c syscall.RawConn) error {
	if n.iface == "" || len(n.iface) > 15 || strings.ContainsAny(n.iface, "/\\\x00") {
		return errors.New("Physical interface required")
	}
	var socketErr error
	e := c.Control(func(fd uintptr) {
		socketErr = unix.SetsockoptString(int(fd), unix.SOL_SOCKET, unix.SO_BINDTODEVICE, n.iface)
		if socketErr == nil {
			mark := tcpMark
			if strings.HasPrefix(network, "udp") {
				mark = udpMark
			}
			socketErr = unix.SetsockoptInt(int(fd), unix.SOL_SOCKET, unix.SO_MARK, mark)
		}
	})
	if e != nil {
		return e
	}
	return socketErr
}
