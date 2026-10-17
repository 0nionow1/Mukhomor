package main

import "syscall"

// VpnService excludes our package UID, including this supervised child.
func (n *underlay) control(network, address string, c syscall.RawConn) error { return nil }
