package main

import (
	"bufio"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"os/signal"
	"syscall"

	"github.com/0nionow1/Mukhomor/core"
)

func main() {
	root := flag.String("root", "", "Private application data directory")
	checkTor := flag.Bool("check-tor-guards", false, "Refuse removal while Tor traffic protection is retained")
	flag.Parse()
	if *checkTor {
		present, err := core.TorGuardsPresent()
		if err != nil {
			fmt.Fprintln(os.Stderr, "Cannot inspect Tor traffic protection")
			os.Exit(2)
		}
		if present {
			fmt.Fprintln(os.Stderr, "Disconnect Tor in Mukhomor before uninstalling")
			os.Exit(1)
		}
		return
	}
	if *root == "" {
		fmt.Fprintln(os.Stderr, "--root is required")
		os.Exit(2)
	}
	c, err := core.New(*root, "linux", nil)
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	defer c.Close()
	signals := make(chan os.Signal, 1)
	signal.Notify(signals, syscall.SIGTERM, syscall.SIGINT, syscall.SIGHUP)
	go func() { <-signals; c.Close(); os.Exit(0) }()
	scanner := bufio.NewScanner(os.Stdin)
	scanner.Buffer(make([]byte, 4096), core.MaxFrame+1)
	out := bufio.NewWriter(os.Stdout)
	for scanner.Scan() {
		reply := c.Request(scanner.Bytes(), -1)
		if _, err = out.Write(reply); err != nil {
			return
		}
		_ = out.WriteByte('\n')
		if out.Flush() != nil {
			return
		}
		var request core.Object
		if json.Unmarshal(scanner.Bytes(), &request) == nil && request["action"] == "Exit" {
			return
		}
	}
}
