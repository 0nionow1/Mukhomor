package main

import (
	"encoding/binary"
	"errors"
	"golang.org/x/sys/windows"
	"strconv"
	"strings"
	"syscall"
)

func (n *underlay) control(network, address string, c syscall.RawConn) error {
	index, e := strconv.ParseUint(n.iface, 10, 32)
	if e != nil || index == 0 {
		return errors.New("Physical interface required")
	}
	var socketErr error
	e = c.Control(func(fd uintptr) {
		level, option, value := windows.IPPROTO_IPV6, 31, uint32(index)
		if strings.HasSuffix(network, "4") {
			level = windows.IPPROTO_IP
			option = 31
			var b [4]byte
			binary.BigEndian.PutUint32(b[:], uint32(index))
			value = binary.NativeEndian.Uint32(b[:])
		}
		socketErr = windows.SetsockoptInt(windows.Handle(fd), level, option, int(value))
	})
	if e != nil {
		return e
	}
	return socketErr
}
