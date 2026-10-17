package main

import (
	"os/exec"
	"syscall"
)

func hideI2PChild(c *exec.Cmd) {
	c.SysProcAttr = &syscall.SysProcAttr{HideWindow: true, CreationFlags: 0x08000000}
}
