package core

import (
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"strings"
	"time"

	assets "github.com/0nionow1/Mukhomor"
)

func (c *Controller) updateRules() error {
	if c.engine.Running() {
		return errors.New("Disconnect before updating rule lists")
	}
	catalogBytes, _ := assets.Files.ReadFile("lists/sources.json")
	var catalog Object
	_ = json.Unmarshal(catalogBytes, &catalog)
	client := &http.Client{Timeout: 20 * time.Second, CheckRedirect: func(r *http.Request, via []*http.Request) error {
		if r.URL.Scheme != "https" || r.URL.Host != "raw.githubusercontent.com" || len(via) > 2 {
			return errors.New("Unexpected rule source redirect")
		}
		return nil
	}}
	lists := Object{}
	for _, name := range []string{"russia", "steam", "ozon", "games"} {
		url := str(obj(obj(catalog["sources"])[name])["download_url"])
		if !strings.HasPrefix(url, "https://raw.githubusercontent.com/MetaCubeX/meta-rules-dat/") {
			return errors.New("Invalid bundled rule source")
		}
		response, err := client.Get(url)
		if err != nil {
			return errors.New("Cannot download rule lists; existing lists are preserved")
		}
		b, err := io.ReadAll(io.LimitReader(response.Body, (1<<20)+1))
		_ = response.Body.Close()
		if err != nil || response.StatusCode != 200 || len(b) > 1<<20 {
			return errors.New("Rule download failed or exceeded 1 MiB")
		}
		lines, err := parseDomainList(string(b))
		if err != nil {
			return err
		}
		lists[name] = strings.Join(lines, "\n")
	}
	next := obj(clone(c.data))
	next["rule_lists"] = lists
	next["rules_updated_at"] = time.Now().UTC().Format(time.RFC3339)
	if err := c.store.Write(next); err != nil {
		return errors.New("Cannot save downloaded rules")
	}
	c.data = next
	return nil
}
func parseDomainList(text string) ([]string, error) {
	lines := []string{}
	seen := map[string]bool{}
	for _, line := range strings.Split(text, "\n") {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		domain := strings.TrimPrefix(line, "+.")
		if len(domain) > 253 || !domainPattern.MatchString(domain) || strings.Contains(domain, "..") {
			return nil, errors.New("Rule source contains an invalid domain")
		}
		if !seen[line] {
			seen[line] = true
			lines = append(lines, line)
		}
	}
	if len(lines) == 0 {
		return nil, errors.New("Rule source is empty")
	}
	return lines, nil
}
