//go:build !linux || android

package core

func newTorGuard() torGuard { return noTorGuard{} }

func TorGuardsPresent() (bool, error) { return false, nil }
