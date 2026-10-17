//go:build !windows

package main

import "os/exec"

func hideI2PChild(c *exec.Cmd) {}
