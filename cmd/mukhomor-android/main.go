//go:build android

package main

/*
#include <stdlib.h>
int protect_socket(int fd);
*/
import "C"

import (
	"encoding/json"
	"errors"
	"sync"
	"syscall"
	"unsafe"

	"github.com/0nionow1/Mukhomor/core"
	"github.com/metacubex/mihomo/component/dialer"
)

var initMu sync.Mutex
var controller *core.Controller

//export mukhomor_request
func mukhomor_request(root *C.char, request *C.char, torPath *C.char, fd C.int) *C.char {
	initMu.Lock()
	if controller == nil {
		dialer.DefaultSocketHook = func(_, _ string, conn syscall.RawConn) error {
			var protected bool
			err := conn.Control(func(fd uintptr) { protected = C.protect_socket(C.int(fd)) != 0 })
			if err != nil {
				return err
			}
			if !protected {
				return errors.New("Android refused socket protection")
			}
			return nil
		}
		var err error
		controller, err = core.New(C.GoString(root), "android", nil)
		if err != nil {
			initMu.Unlock()
			b, _ := json.Marshal(map[string]any{"ok": false, "error": err.Error()})
			return C.CString(string(b))
		}
		controller.SetTorHelperPath(C.GoString(torPath))
	}
	c := controller
	initMu.Unlock()
	return C.CString(string(c.Request([]byte(C.GoString(request)), int(fd))))
}

//export mukhomor_packages
func mukhomor_packages() *C.char {
	initMu.Lock()
	c := controller
	initMu.Unlock()
	if c == nil {
		return C.CString("[]")
	}
	packages := c.AndroidPackages()
	if packages == nil {
		packages = []string{}
	}
	b, _ := json.Marshal(packages)
	return C.CString(string(b))
}

//export mukhomor_free
func mukhomor_free(p unsafe.Pointer) { C.free(p) }
func main()                          {}
