package zanshin

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"
)

// Option configures a Client.
type Option func(*Client)

// WithHTTPClient replaces the default HTTP client.
func WithHTTPClient(h *http.Client) Option { return func(c *Client) { c.http = h } }

// WithCacheTTL sets how long per-identity evaluations are served from the local cache
// when no invalidation arrives. Zero disables caching.
func WithCacheTTL(d time.Duration) Option { return func(c *Client) { c.cacheTTL = d } }

// WithLogger receives connection and cache diagnostics.
func WithLogger(l func(format string, args ...any)) Option { return func(c *Client) { c.logf = l } }

// Client talks to one Zanshin server with a server (or admin) API key.
type Client struct {
	baseURL  string
	apiKey   string
	http     *http.Client
	cacheTTL time.Duration
	logf     func(string, ...any)

	mu    sync.Mutex
	cache map[string]cacheEntry // key: kind + "\x00" + externalID + "\x00" + args
	gen   uint64

	streamCancel context.CancelFunc
}

type cacheEntry struct {
	at   time.Time
	gen  uint64
	data []byte
}

// New creates a client. baseURL is the REST address, e.g. "http://zanshin:7550".
func New(baseURL, serverKey string, opts ...Option) *Client {
	c := &Client{
		baseURL:  strings.TrimRight(baseURL, "/"),
		apiKey:   serverKey,
		http:     &http.Client{Timeout: 10 * time.Second},
		cacheTTL: 30 * time.Second,
		logf:     func(string, ...any) {},
		cache:    map[string]cacheEntry{},
	}
	for _, o := range opts {
		o(c)
	}
	return c
}

// Close stops the change stream, if running.
func (c *Client) Close() {
	c.mu.Lock()
	cancel := c.streamCancel
	c.streamCancel = nil
	c.mu.Unlock()
	if cancel != nil {
		cancel()
	}
}

// Invalidate drops every cached evaluation.
func (c *Client) Invalidate() {
	c.mu.Lock()
	c.gen++
	c.cache = map[string]cacheEntry{}
	c.mu.Unlock()
}

// InvalidateIdentity drops cached evaluations of one identity.
func (c *Client) InvalidateIdentity(externalID string) {
	c.mu.Lock()
	for k := range c.cache {
		parts := strings.SplitN(k, "\x00", 3)
		if len(parts) >= 2 && parts[1] == externalID {
			delete(c.cache, k)
		}
	}
	c.mu.Unlock()
}

func (c *Client) do(ctx context.Context, method, path string, query url.Values, body any, out any) error {
	u := c.baseURL + path
	if len(query) > 0 {
		u += "?" + query.Encode()
	}
	var rd io.Reader
	if body != nil {
		b, err := json.Marshal(body)
		if err != nil {
			return err
		}
		rd = bytes.NewReader(b)
	}
	req, err := http.NewRequestWithContext(ctx, method, u, rd)
	if err != nil {
		return err
	}
	req.Header.Set("x-api-key", c.apiKey)
	req.Header.Set("accept", "application/json")
	if body != nil {
		req.Header.Set("content-type", "application/json")
	}
	res, err := c.http.Do(req)
	if err != nil {
		return err
	}
	defer res.Body.Close()
	data, err := io.ReadAll(io.LimitReader(res.Body, 16<<20))
	if err != nil {
		return err
	}
	if res.StatusCode >= 400 {
		apiErr := &Error{Status: res.StatusCode}
		if json.Unmarshal(data, apiErr) != nil || apiErr.Code == "" {
			apiErr.Code = http.StatusText(res.StatusCode)
			apiErr.Message = strings.TrimSpace(string(data))
		}
		return apiErr
	}
	if out != nil && len(data) > 0 {
		return json.Unmarshal(data, out)
	}
	return nil
}

// cached runs fetch unless a fresh cache entry exists for (kind, externalID, args).
func (c *Client) cached(ctx context.Context, kind, externalID, args string, out any, fetch func() ([]byte, error)) error {
	key := kind + "\x00" + externalID + "\x00" + args
	if c.cacheTTL > 0 {
		c.mu.Lock()
		e, ok := c.cache[key]
		gen := c.gen
		c.mu.Unlock()
		if ok && e.gen == gen && time.Since(e.at) < c.cacheTTL {
			return json.Unmarshal(e.data, out)
		}
	}
	data, err := fetch()
	if err != nil {
		return err
	}
	if c.cacheTTL > 0 {
		c.mu.Lock()
		c.cache[key] = cacheEntry{at: time.Now(), gen: c.gen, data: data}
		c.mu.Unlock()
	}
	return json.Unmarshal(data, out)
}

func (c *Client) fetchJSON(ctx context.Context, path string, q url.Values) ([]byte, error) {
	var raw json.RawMessage
	if err := c.do(ctx, http.MethodGet, path, q, nil, &raw); err != nil {
		return nil, err
	}
	return raw, nil
}

func idQuery(externalID string, names []string) url.Values {
	q := url.Values{"external_id": {externalID}}
	if len(names) > 0 {
		q.Set("names", strings.Join(names, ","))
	}
	return q
}

// UpsertIdentity creates or refreshes an identity (call it on login). Properties are merged.
func (c *Client) UpsertIdentity(ctx context.Context, externalID string, defaultProps, customProps map[string]any) (*Identity, error) {
	var out Identity
	body := map[string]any{"external_id": externalID}
	if defaultProps != nil {
		body["default"] = defaultProps
	}
	if customProps != nil {
		body["custom"] = customProps
	}
	if err := c.do(ctx, http.MethodPost, "/server/v1/identities", nil, body, &out); err != nil {
		return nil, err
	}
	c.InvalidateIdentity(externalID)
	return &out, nil
}

// UpdateProperties merges properties of an existing identity (JSON null removes a key).
func (c *Client) UpdateProperties(ctx context.Context, externalID string, defaultProps, customProps map[string]any) (*Properties, error) {
	var out Properties
	body := map[string]any{}
	if defaultProps != nil {
		body["default"] = defaultProps
	}
	if customProps != nil {
		body["custom"] = customProps
	}
	if err := c.do(ctx, http.MethodPut, "/server/v1/identities/"+url.PathEscape(externalID)+"/properties", nil, body, &out); err != nil {
		return nil, err
	}
	c.InvalidateIdentity(externalID)
	return &out, nil
}

// Flags evaluates feature flags for an identity (all when names is empty).
func (c *Client) Flags(ctx context.Context, externalID string, names ...string) ([]Flag, error) {
	var out struct {
		Flags []Flag `json:"flags"`
	}
	err := c.cached(ctx, "flags", externalID, strings.Join(names, ","), &out, func() ([]byte, error) {
		return c.fetchJSON(ctx, "/server/v1/flags", idQuery(externalID, names))
	})
	return out.Flags, err
}

// Flag evaluates one flag. Returns a *Error with Status 404 when the flag does not exist.
func (c *Client) Flag(ctx context.Context, externalID, name string) (*Flag, error) {
	flags, err := c.Flags(ctx, externalID, name)
	if err != nil {
		return nil, err
	}
	for i := range flags {
		if flags[i].Key == name {
			return &flags[i], nil
		}
	}
	return nil, &Error{Status: 404, Code: "not_found", Message: "flag " + name + " not found"}
}

// Audiences lists the audiences an identity belongs to.
func (c *Client) Audiences(ctx context.Context, externalID string) ([]string, error) {
	var out struct {
		Audiences []string `json:"audiences"`
	}
	err := c.cached(ctx, "audiences", externalID, "", &out, func() ([]byte, error) {
		return c.fetchJSON(ctx, "/server/v1/audiences", idQuery(externalID, nil))
	})
	return out.Audiences, err
}

// Experiments returns the identity's enrolments, assigning it where newly eligible.
func (c *Client) Experiments(ctx context.Context, externalID string, names ...string) ([]Experiment, error) {
	var out struct {
		Experiments []Experiment `json:"experiments"`
	}
	err := c.cached(ctx, "experiments", externalID, strings.Join(names, ","), &out, func() ([]byte, error) {
		return c.fetchJSON(ctx, "/server/v1/experiments", idQuery(externalID, names))
	})
	return out.Experiments, err
}

// Expose records that the identity saw its variant of an experiment.
func (c *Client) Expose(ctx context.Context, externalID, experiment string) error {
	return c.do(ctx, http.MethodPost, "/server/v1/experiments/"+url.PathEscape(experiment)+"/expose", idQuery(externalID, nil), nil, nil)
}

// LiveEvents returns live events targeting the identity; upcoming adds inactive ones with their next occurrence.
func (c *Client) LiveEvents(ctx context.Context, externalID string, upcoming bool, names ...string) ([]LiveEvent, error) {
	var out struct {
		LiveEvents []LiveEvent `json:"live_events"`
	}
	q := idQuery(externalID, names)
	if upcoming {
		q.Set("upcoming", "true")
	}
	// Activity depends on wall-clock time: cache briefly only.
	err := c.cached(ctx, "live_events", externalID, q.Encode(), &out, func() ([]byte, error) {
		return c.fetchJSON(ctx, "/server/v1/live-events", q)
	})
	return out.LiveEvents, err
}

// Inbox delivers pending campaigns and returns the identity's unconsumed messages.
func (c *Client) Inbox(ctx context.Context, externalID string, limit int) ([]InboxMessage, error) {
	var out struct {
		Messages []InboxMessage `json:"messages"`
	}
	q := idQuery(externalID, nil)
	if limit > 0 {
		q.Set("limit", fmt.Sprint(limit))
	}
	err := c.do(ctx, http.MethodGet, "/server/v1/messages/inbox", q, nil, &out)
	return out.Messages, err
}

// SendMessage drops a direct message into the identity's inbox and returns its id.
func (c *Client) SendMessage(ctx context.Context, externalID string, m DirectMessage) (string, error) {
	body := map[string]any{"external_id": externalID, "title": m.Title, "body": m.Body, "image_url": m.ImageURL, "priority": m.Priority}
	if m.Payload != nil {
		body["payload"] = m.Payload
	}
	if m.TTL != nil {
		body["ttl_secs"] = int64(m.TTL.Seconds())
	}
	var out struct {
		ID string `json:"id"`
	}
	err := c.do(ctx, http.MethodPost, "/server/v1/messages", nil, body, &out)
	return out.ID, err
}

// Track reports analytics events for an identity and returns how many were accepted.
func (c *Client) Track(ctx context.Context, externalID string, events ...Event) (int, error) {
	if len(events) == 0 {
		return 0, nil
	}
	var out struct {
		Accepted int `json:"accepted"`
	}
	err := c.do(ctx, http.MethodPost, "/server/v1/events", nil, map[string]any{"external_id": externalID, "events": events}, &out)
	return out.Accepted, err
}

// Identify links externalID to a new external id (e.g. an anonymous device id to the account that
// just signed in). If newID already exists the two identities are merged into it. Cached
// evaluations for both ids are dropped.
func (c *Client) Identify(ctx context.Context, externalID, newID string) (*Identity, error) {
	var out Identity
	err := c.do(ctx, http.MethodPost, "/server/v1/identities/"+url.PathEscape(externalID)+"/identify", nil, map[string]any{"id": newID}, &out)
	if err != nil {
		return nil, err
	}
	c.InvalidateIdentity(externalID)
	c.InvalidateIdentity(newID)
	return &out, nil
}

// AddAudienceMembers adds identities (by external id) to a managed audience.
func (c *Client) AddAudienceMembers(ctx context.Context, audience string, externalIDs ...string) error {
	return c.audienceMembers(ctx, http.MethodPost, audience, externalIDs)
}

// RemoveAudienceMembers removes identities (by external id) from a managed audience.
func (c *Client) RemoveAudienceMembers(ctx context.Context, audience string, externalIDs ...string) error {
	return c.audienceMembers(ctx, http.MethodDelete, audience, externalIDs)
}

func (c *Client) audienceMembers(ctx context.Context, method, audience string, externalIDs []string) error {
	if err := c.do(ctx, method, "/server/v1/audiences/"+url.PathEscape(audience)+"/members", nil, map[string]any{"external_ids": externalIDs}, nil); err != nil {
		return err
	}
	for _, id := range externalIDs {
		c.InvalidateIdentity(id)
	}
	return nil
}

// ExplainFlag returns the full override chain (identity override, experiment, live event, rules,
// default) the server walked to evaluate name for the identity. Meant for debugging / support tooling.
func (c *Client) ExplainFlag(ctx context.Context, externalID, name string) (json.RawMessage, error) {
	var out json.RawMessage
	if err := c.do(ctx, http.MethodGet, "/server/v1/flags/"+url.PathEscape(name)+"/explain", idQuery(externalID, nil), nil, &out); err != nil {
		return nil, err
	}
	return out, nil
}

// JoinLiveEvent explicitly joins the identity to a live event with join_mode "explicit" and
// returns the event as the identity now sees it.
func (c *Client) JoinLiveEvent(ctx context.Context, externalID, event string) (*LiveEvent, error) {
	var out LiveEvent
	if err := c.do(ctx, http.MethodPost, "/server/v1/live-events/"+url.PathEscape(event)+"/join", idQuery(externalID, nil), nil, &out); err != nil {
		return nil, err
	}
	c.InvalidateIdentity(externalID)
	return &out, nil
}

// LeaveLiveEvent removes the identity from a live event; false means it was not a participant.
func (c *Client) LeaveLiveEvent(ctx context.Context, externalID, event string) (bool, error) {
	var out struct {
		Left bool `json:"left"`
	}
	if err := c.do(ctx, http.MethodPost, "/server/v1/live-events/"+url.PathEscape(event)+"/leave", idQuery(externalID, nil), nil, &out); err != nil {
		return false, err
	}
	c.InvalidateIdentity(externalID)
	return out.Left, nil
}

// MarkRead marks an inbox message read and returns it.
func (c *Client) MarkRead(ctx context.Context, externalID, messageID string) (*InboxMessage, error) {
	var out InboxMessage
	if err := c.do(ctx, http.MethodPut, "/server/v1/messages/inbox/"+url.PathEscape(messageID), idQuery(externalID, nil), nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// Consume consumes an inbox message: it leaves the inbox and is never redelivered.
func (c *Client) Consume(ctx context.Context, externalID, messageID string) error {
	return c.do(ctx, http.MethodPost, "/server/v1/messages/inbox/"+url.PathEscape(messageID)+"/consume", idQuery(externalID, nil), nil, nil)
}

// DeleteInboxMessage removes a message from the inbox entirely.
func (c *Client) DeleteInboxMessage(ctx context.Context, externalID, messageID string) error {
	return c.do(ctx, http.MethodDelete, "/server/v1/messages/inbox/"+url.PathEscape(messageID), idQuery(externalID, nil), nil, nil)
}
