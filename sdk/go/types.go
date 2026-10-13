// Package zanshin is the Go client for a Zanshin server, meant for trusted backends
// (game servers, Nakama runtime modules). It talks to the server-key API, caches
// per-identity evaluations locally and invalidates them from the server change stream.
package zanshin

import (
	"encoding/json"
	"time"
)

// Flag is an evaluated feature flag for one identity.
type Flag struct {
	Key       string          `json:"key"`
	Value     json.RawMessage `json:"value"`
	ValueType string          `json:"value_type"`
	Variant   string          `json:"variant,omitempty"`
	Reason    Reason          `json:"reason"`
	Version   int64           `json:"version"`
}

// Reason explains where a flag value came from.
type Reason struct {
	Kind       string `json:"kind"` // disabled | default | override | experiment | rule | variant
	RuleID     string `json:"rule_id,omitempty"`
	Variant    string `json:"variant,omitempty"`
	Experiment string `json:"experiment,omitempty"`
}

// Bool returns the value as bool (false when it is not a bool).
func (f Flag) Bool() bool {
	var b bool
	_ = json.Unmarshal(f.Value, &b)
	return b
}

// String returns the value as string (empty when it is not a string).
func (f Flag) String() string {
	var s string
	_ = json.Unmarshal(f.Value, &s)
	return s
}

// Float returns the value as float64 (0 when it is not a number).
func (f Flag) Float() float64 {
	var n float64
	_ = json.Unmarshal(f.Value, &n)
	return n
}

// Decode unmarshals a JSON flag value into v.
func (f Flag) Decode(v any) error { return json.Unmarshal(f.Value, v) }

// Experiment is an identity's enrolment.
type Experiment struct {
	Key     string          `json:"key"`
	Name    string          `json:"name"`
	Variant string          `json:"variant"`
	Value   json.RawMessage `json:"value"`
	Version int64           `json:"version"`
}

// Occurrence is one live event window.
type Occurrence struct {
	Start time.Time `json:"start"`
	End   time.Time `json:"end"`
}

// LiveEvent is a live event as seen by one identity.
type LiveEvent struct {
	Key         string          `json:"key"`
	Name        string          `json:"name"`
	Description string          `json:"description"`
	Value       json.RawMessage `json:"value"`
	Active      bool            `json:"active"`
	Current     *Occurrence     `json:"current,omitempty"`
	Next        *Occurrence     `json:"next,omitempty"`
	Version     int64           `json:"version"`
}

// InboxMessage is a delivered message.
type InboxMessage struct {
	ID          string          `json:"id"`
	Key         *string         `json:"key"`
	Title       string          `json:"title"`
	Body        string          `json:"body"`
	ImageURL    string          `json:"image_url"`
	Payload     json.RawMessage `json:"payload"`
	Priority    int32           `json:"priority"`
	DeliveredAt time.Time       `json:"delivered_at"`
	ReadAt      *time.Time      `json:"read_at"`
	ExpiresAt   *time.Time      `json:"expires_at"`
}

// DirectMessage is a one-off message sent into an identity's inbox.
type DirectMessage struct {
	Title    string         `json:"title"`
	Body     string         `json:"body,omitempty"`
	ImageURL string         `json:"image_url,omitempty"`
	Payload  any            `json:"payload,omitempty"`
	Priority int32          `json:"priority,omitempty"`
	TTL      *time.Duration `json:"-"`
}

// Event is an analytics event.
type Event struct {
	Name      string         `json:"name"`
	Value     string         `json:"value,omitempty"`
	Metadata  map[string]any `json:"metadata,omitempty"`
	Timestamp *time.Time     `json:"timestamp,omitempty"`
	ID        string         `json:"id,omitempty"`
}

// Properties are the three property namespaces of an identity.
type Properties struct {
	Default  map[string]any `json:"default"`
	Computed map[string]any `json:"computed"`
	Custom   map[string]any `json:"custom"`
}

// Identity is a Zanshin identity.
type Identity struct {
	ID         string     `json:"id"`
	ExternalID string     `json:"external_id"`
	CreatedAt  time.Time  `json:"created_at"`
	UpdatedAt  time.Time  `json:"updated_at"`
	LastSeenAt time.Time  `json:"last_seen_at"`
	Properties Properties `json:"properties"`
}

// Change is one entry of the server change stream.
type Change struct {
	Type       string  `json:"type"` // hello | change | resync | pong
	Kind       string  `json:"kind,omitempty"`
	Key        string  `json:"key,omitempty"`
	Version    int64   `json:"version,omitempty"`
	Deleted    bool    `json:"deleted,omitempty"`
	IdentityID *string `json:"identity_id,omitempty"`
}

// Error is a structured API error.
type Error struct {
	Status  int    `json:"-"`
	Code    string `json:"code"`
	Message string `json:"message"`
}

func (e *Error) Error() string { return e.Code + ": " + e.Message }

// IsNotFound reports whether err is a 404 from the server.
func IsNotFound(err error) bool {
	e, ok := err.(*Error)
	return ok && e.Status == 404
}
