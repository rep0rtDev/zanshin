package zanshin

import (
	"context"
	"encoding/json"
	"strings"
	"sync"
	"time"

	"github.com/rep0rtDev/zanshin/sdk/go/local"
)

// WithLocalEvaluation makes LocalFlags evaluate flags in-process from the rules bundle
// (GET /server/v1/rules), refreshed every ttl and whenever the change stream reports a change.
// Overrides, experiments and live events are not applied locally — use Flags for those.
func WithLocalEvaluation(ttl time.Duration) Option {
	return func(c *Client) {
		if ttl <= 0 {
			ttl = 60 * time.Second
		}
		c.rules = &rulesCache{ttl: ttl}
	}
}

type rulesCache struct {
	mu   sync.Mutex
	ttl  time.Duration
	at   time.Time
	gen  uint64
	eval *local.Evaluator
}

// Rules fetches (or returns the cached) local evaluator. Works without WithLocalEvaluation too,
// in which case the bundle is fetched on every call.
func (c *Client) Rules(ctx context.Context) (*local.Evaluator, error) {
	if c.rules == nil {
		return c.fetchRules(ctx)
	}
	c.mu.Lock()
	gen := c.gen
	c.mu.Unlock()
	c.rules.mu.Lock()
	defer c.rules.mu.Unlock()
	if c.rules.eval != nil && c.rules.gen == gen && time.Since(c.rules.at) < c.rules.ttl {
		return c.rules.eval, nil
	}
	e, err := c.fetchRules(ctx)
	if err != nil {
		if c.rules.eval != nil {
			c.logf("zanshin: rules refresh failed, serving stale bundle: %v", err)
			return c.rules.eval, nil
		}
		return nil, err
	}
	c.rules.eval, c.rules.at, c.rules.gen = e, time.Now(), gen
	return e, nil
}

func (c *Client) fetchRules(ctx context.Context) (*local.Evaluator, error) {
	var b local.Bundle
	if err := c.do(ctx, "GET", "/server/v1/rules", nil, nil, &b); err != nil {
		return nil, err
	}
	return local.New(&b), nil
}

// LocalFlags evaluates flags in-process for externalID with the given property set
// ({"default": {...}, "custom": {...}}); props may be nil. Pass names to limit the result.
func (c *Client) LocalFlags(ctx context.Context, externalID string, props map[string]map[string]any, names ...string) ([]Flag, error) {
	e, err := c.Rules(ctx)
	if err != nil {
		return nil, err
	}
	results := e.EvaluateAll(externalID, local.Flatten(externalID, props), names...)
	out := make([]Flag, 0, len(results))
	for _, r := range results {
		var f Flag
		raw, _ := json.Marshal(r)
		if json.Unmarshal(raw, &f) == nil {
			out = append(out, f)
		}
	}
	return out, nil
}

// LocalFlag evaluates one flag in-process. Returns a *Error with Status 404 for unknown flags.
func (c *Client) LocalFlag(ctx context.Context, externalID, name string, props map[string]map[string]any) (*Flag, error) {
	flags, err := c.LocalFlags(ctx, externalID, props, name)
	if err != nil {
		return nil, err
	}
	if len(flags) == 0 || !strings.EqualFold(flags[0].Key, name) {
		return nil, &Error{Status: 404, Code: "not_found", Message: "flag " + name + " not found"}
	}
	return &flags[0], nil
}
