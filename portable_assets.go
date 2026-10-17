// Package assets contains the same reviewed defaults used by the Windows client.
package assets

import "embed"

//go:embed assets/settings.default.json lists/seed-*.list lists/sources.json
var Files embed.FS

const Version = "0.8.8"
