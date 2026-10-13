// Zanshin runtime module for Nakama: syncs identities on login and exposes LiveOps
// features to clients through RPCs, evaluated against the Zanshin server with a local cache.
//
// Configuration (environment of the Nakama process):
//
//	ZANSHIN_URL         base URL of the Zanshin REST API (default http://zanshin:7550)
//	ZANSHIN_SERVER_KEY  server API key (required)
//	ZANSHIN_CACHE_TTL   cache TTL for per-user evaluations (default 30s)
package main

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"strings"
	"time"

	"github.com/heroiclabs/nakama-common/api"
	"github.com/heroiclabs/nakama-common/runtime"
	zanshin "github.com/rep0rtDev/zanshin/sdk/go"
)

const (
	rpcFlags       = "zanshin.flags"
	rpcAudiences   = "zanshin.audiences"
	rpcExperiments = "zanshin.experiments"
	rpcExpose      = "zanshin.expose"
	rpcLiveEvents  = "zanshin.live_events"
	rpcInbox       = "zanshin.inbox"
	rpcTrack       = "zanshin.track"
	rpcProperties  = "zanshin.properties"
)

var errNoUser = runtime.NewError("no user in context", 16) // UNAUTHENTICATED

// InitModule is the Nakama plugin entry point.
func InitModule(ctx context.Context, logger runtime.Logger, db *sql.DB, nk runtime.NakamaModule, initializer runtime.Initializer) error {
	key := os.Getenv("ZANSHIN_SERVER_KEY")
	if key == "" {
		return errors.New("ZANSHIN_SERVER_KEY is not set")
	}
	base := os.Getenv("ZANSHIN_URL")
	if base == "" {
		base = "http://zanshin:7550"
	}
	ttl := 30 * time.Second
	if v := os.Getenv("ZANSHIN_CACHE_TTL"); v != "" {
		if d, err := time.ParseDuration(v); err == nil {
			ttl = d
		}
	}
	client := zanshin.New(base, key, zanshin.WithCacheTTL(ttl), zanshin.WithLogger(func(f string, a ...any) { logger.Info(f, a...) }))
	client.StartStream(ctx)
	m := &module{z: client, logger: logger}

	for name, fn := range map[string]func(context.Context, runtime.Logger, *sql.DB, runtime.NakamaModule, string) (string, error){
		rpcFlags:       m.rpcFlags,
		rpcAudiences:   m.rpcAudiences,
		rpcExperiments: m.rpcExperiments,
		rpcExpose:      m.rpcExpose,
		rpcLiveEvents:  m.rpcLiveEvents,
		rpcInbox:       m.rpcInbox,
		rpcTrack:       m.rpcTrack,
		rpcProperties:  m.rpcProperties,
	} {
		if err := initializer.RegisterRpc(name, fn); err != nil {
			return err
		}
	}

	// Keep Zanshin identities in sync with Nakama accounts on every login.
	if err := initializer.RegisterAfterAuthenticateDevice(func(ctx context.Context, logger runtime.Logger, db *sql.DB, nk runtime.NakamaModule, out *api.Session, in *api.AuthenticateDeviceRequest) error {
		return m.afterAuth(ctx, nk, "device")
	}); err != nil {
		return err
	}
	if err := initializer.RegisterAfterAuthenticateEmail(func(ctx context.Context, logger runtime.Logger, db *sql.DB, nk runtime.NakamaModule, out *api.Session, in *api.AuthenticateEmailRequest) error {
		return m.afterAuth(ctx, nk, "email")
	}); err != nil {
		return err
	}
	if err := initializer.RegisterAfterAuthenticateCustom(func(ctx context.Context, logger runtime.Logger, db *sql.DB, nk runtime.NakamaModule, out *api.Session, in *api.AuthenticateCustomRequest) error {
		return m.afterAuth(ctx, nk, "custom")
	}); err != nil {
		return err
	}
	if err := initializer.RegisterAfterAuthenticateGoogle(func(ctx context.Context, logger runtime.Logger, db *sql.DB, nk runtime.NakamaModule, out *api.Session, in *api.AuthenticateGoogleRequest) error {
		return m.afterAuth(ctx, nk, "google")
	}); err != nil {
		return err
	}
	if err := initializer.RegisterAfterAuthenticateApple(func(ctx context.Context, logger runtime.Logger, db *sql.DB, nk runtime.NakamaModule, out *api.Session, in *api.AuthenticateAppleRequest) error {
		return m.afterAuth(ctx, nk, "apple")
	}); err != nil {
		return err
	}
	if err := initializer.RegisterAfterAuthenticateSteam(func(ctx context.Context, logger runtime.Logger, db *sql.DB, nk runtime.NakamaModule, out *api.Session, in *api.AuthenticateSteamRequest) error {
		return m.afterAuth(ctx, nk, "steam")
	}); err != nil {
		return err
	}

	logger.Info("zanshin module loaded (server %s, cache ttl %s)", base, ttl)
	return nil
}

type module struct {
	z      *zanshin.Client
	logger runtime.Logger
}

func userID(ctx context.Context) (string, error) {
	id, ok := ctx.Value(runtime.RUNTIME_CTX_USER_ID).(string)
	if !ok || id == "" {
		return "", errNoUser
	}
	return id, nil
}

// afterAuth upserts the Zanshin identity with account facts as default properties.
func (m *module) afterAuth(ctx context.Context, nk runtime.NakamaModule, provider string) error {
	uid, err := userID(ctx)
	if err != nil {
		return nil // never block login
	}
	props := map[string]any{"auth_provider": provider}
	if v, ok := ctx.Value(runtime.RUNTIME_CTX_CLIENT_IP).(string); ok && v != "" {
		props["client_ip"] = v
	}
	if v, ok := ctx.Value(runtime.RUNTIME_CTX_LANG).(string); ok && v != "" {
		props["lang"] = v
	}
	if acc, err := nk.AccountGetId(ctx, uid); err == nil && acc.User != nil {
		props["username"] = acc.User.Username
		if acc.User.LangTag != "" {
			props["lang"] = acc.User.LangTag
		}
		if acc.User.Location != "" {
			props["location"] = acc.User.Location
		}
		if acc.User.Timezone != "" {
			props["timezone"] = acc.User.Timezone
		}
		props["created_at"] = acc.User.CreateTime.AsTime().UTC().Format(time.RFC3339)
	}
	go func() {
		bg, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		if _, err := m.z.UpsertIdentity(bg, uid, props, nil); err != nil {
			m.logger.Warn("zanshin: identity sync failed for %s: %v", uid, err)
		}
	}()
	return nil
}

func rpcErr(err error) error {
	var apiErr *zanshin.Error
	if errors.As(err, &apiErr) {
		code := 13 // INTERNAL
		switch apiErr.Status {
		case 400:
			code = 3
		case 401:
			code = 16
		case 403:
			code = 7
		case 404:
			code = 5
		case 409:
			code = 6
		case 503:
			code = 14
		}
		return runtime.NewError(apiErr.Message, code)
	}
	return runtime.NewError(err.Error(), 13)
}

func respond(v any) (string, error) {
	b, err := json.Marshal(v)
	if err != nil {
		return "", runtime.NewError(err.Error(), 13)
	}
	return string(b), nil
}

type namesPayload struct {
	Names    []string `json:"names"`
	Upcoming bool     `json:"upcoming"`
	Limit    int      `json:"limit"`
}

func parseNames(payload string) namesPayload {
	var p namesPayload
	if strings.TrimSpace(payload) != "" {
		_ = json.Unmarshal([]byte(payload), &p)
	}
	return p
}

// rpcFlags: payload {"names":["a","b"]} → {"flags":[…]}
func (m *module) rpcFlags(ctx context.Context, _ runtime.Logger, _ *sql.DB, _ runtime.NakamaModule, payload string) (string, error) {
	uid, err := userID(ctx)
	if err != nil {
		return "", err
	}
	flags, err := m.z.Flags(ctx, uid, parseNames(payload).Names...)
	if err != nil {
		return "", rpcErr(err)
	}
	return respond(map[string]any{"flags": flags})
}

func (m *module) rpcAudiences(ctx context.Context, _ runtime.Logger, _ *sql.DB, _ runtime.NakamaModule, _ string) (string, error) {
	uid, err := userID(ctx)
	if err != nil {
		return "", err
	}
	a, err := m.z.Audiences(ctx, uid)
	if err != nil {
		return "", rpcErr(err)
	}
	return respond(map[string]any{"audiences": a})
}

func (m *module) rpcExperiments(ctx context.Context, _ runtime.Logger, _ *sql.DB, _ runtime.NakamaModule, payload string) (string, error) {
	uid, err := userID(ctx)
	if err != nil {
		return "", err
	}
	e, err := m.z.Experiments(ctx, uid, parseNames(payload).Names...)
	if err != nil {
		return "", rpcErr(err)
	}
	return respond(map[string]any{"experiments": e})
}

// rpcExpose: payload {"experiment":"price_test"}
func (m *module) rpcExpose(ctx context.Context, _ runtime.Logger, _ *sql.DB, _ runtime.NakamaModule, payload string) (string, error) {
	uid, err := userID(ctx)
	if err != nil {
		return "", err
	}
	var p struct {
		Experiment string `json:"experiment"`
	}
	if json.Unmarshal([]byte(payload), &p) != nil || p.Experiment == "" {
		return "", runtime.NewError("experiment is required", 3)
	}
	if err := m.z.Expose(ctx, uid, p.Experiment); err != nil {
		return "", rpcErr(err)
	}
	return "{}", nil
}

func (m *module) rpcLiveEvents(ctx context.Context, _ runtime.Logger, _ *sql.DB, _ runtime.NakamaModule, payload string) (string, error) {
	uid, err := userID(ctx)
	if err != nil {
		return "", err
	}
	p := parseNames(payload)
	e, err := m.z.LiveEvents(ctx, uid, p.Upcoming, p.Names...)
	if err != nil {
		return "", rpcErr(err)
	}
	return respond(map[string]any{"live_events": e})
}

func (m *module) rpcInbox(ctx context.Context, _ runtime.Logger, _ *sql.DB, _ runtime.NakamaModule, payload string) (string, error) {
	uid, err := userID(ctx)
	if err != nil {
		return "", err
	}
	msgs, err := m.z.Inbox(ctx, uid, parseNames(payload).Limit)
	if err != nil {
		return "", rpcErr(err)
	}
	return respond(map[string]any{"messages": msgs})
}

// rpcTrack: payload {"events":[{"name":"purchase","value":"9.99","metadata":{…}}]}
func (m *module) rpcTrack(ctx context.Context, _ runtime.Logger, _ *sql.DB, _ runtime.NakamaModule, payload string) (string, error) {
	uid, err := userID(ctx)
	if err != nil {
		return "", err
	}
	var p struct {
		Events []zanshin.Event `json:"events"`
	}
	if json.Unmarshal([]byte(payload), &p) != nil || len(p.Events) == 0 {
		return "", runtime.NewError("events are required", 3)
	}
	for _, e := range p.Events {
		if strings.HasPrefix(e.Name, "$") {
			return "", runtime.NewError(fmt.Sprintf("event name %q is reserved", e.Name), 3)
		}
	}
	n, err := m.z.Track(ctx, uid, p.Events...)
	if err != nil {
		return "", rpcErr(err)
	}
	return respond(map[string]any{"accepted": n})
}

// rpcProperties: payload {"custom":{"nickname":"Neo"}} merges custom properties (default/computed are server-owned).
func (m *module) rpcProperties(ctx context.Context, _ runtime.Logger, _ *sql.DB, _ runtime.NakamaModule, payload string) (string, error) {
	uid, err := userID(ctx)
	if err != nil {
		return "", err
	}
	var p struct {
		Custom map[string]any `json:"custom"`
	}
	if json.Unmarshal([]byte(payload), &p) != nil || len(p.Custom) == 0 {
		return "", runtime.NewError("custom properties are required", 3)
	}
	props, err := m.z.UpdateProperties(ctx, uid, nil, p.Custom)
	if err != nil {
		return "", rpcErr(err)
	}
	return respond(props)
}
