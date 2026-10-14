package zanshin

import (
	"context"
	"os"
	"strings"
	"testing"
	"time"
)

// Integration test against a running server: ZANSHIN_TEST_URL and ZANSHIN_TEST_SERVER_KEY.
func TestAgainstServer(t *testing.T) {
	base, key := os.Getenv("ZANSHIN_TEST_URL"), os.Getenv("ZANSHIN_TEST_SERVER_KEY")
	if base == "" || key == "" {
		t.Skip("ZANSHIN_TEST_URL / ZANSHIN_TEST_SERVER_KEY not set")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	logs := make(chan string, 16)
	z := New(base, key, WithLogger(func(f string, a ...any) {
		select {
		case logs <- f:
		default:
		}
	}))
	z.StartStream(ctx)
	defer z.Close()

	uid := "go-sdk-" + time.Now().UTC().Format("150405.000")
	id, err := z.UpsertIdentity(ctx, uid, map[string]any{"platform": "test"}, map[string]any{"nick": "Neo"})
	if err != nil {
		t.Fatalf("upsert: %v", err)
	}
	if id.ExternalID != uid || id.Properties.Default["platform"] != "test" {
		t.Fatalf("unexpected identity %+v", id)
	}
	if _, err := z.Flags(ctx, uid); err != nil {
		t.Fatalf("flags: %v", err)
	}
	if _, err := z.Flag(ctx, uid, "definitely_missing_flag"); !IsNotFound(err) {
		t.Fatalf("expected not found, got %v", err)
	}
	if _, err := z.Audiences(ctx, uid); err != nil {
		t.Fatalf("audiences: %v", err)
	}
	if _, err := z.ExplainFlag(ctx, uid, "definitely_missing_flag"); !IsNotFound(err) {
		t.Fatalf("explain: expected not found, got %v", err)
	}
	if _, err := z.LeaveLiveEvent(ctx, uid, "definitely_missing_event"); !IsNotFound(err) {
		t.Fatalf("leave: expected not found, got %v", err)
	}
	if err := z.DeleteInboxMessage(ctx, uid, "00000000-0000-0000-0000-000000000000"); err == nil {
		t.Fatalf("delete inbox: expected an error for an unknown message")
	}
	linked := uid + "-account"
	if id, err := z.Identify(ctx, uid, linked); err != nil || id.ExternalID != linked {
		t.Fatalf("identify: %v %+v", err, id)
	}
	uid = linked
	if _, err := z.Flags(ctx, uid); err != nil {
		t.Fatalf("flags after identify: %v", err)
	}
	if _, err := z.Experiments(ctx, uid); err != nil {
		t.Fatalf("experiments: %v", err)
	}
	if _, err := z.LiveEvents(ctx, uid, true); err != nil {
		t.Fatalf("live events: %v", err)
	}
	n, err := z.Track(ctx, uid, Event{Name: "sdk.ping", Value: "1", Metadata: map[string]any{"k": "v"}})
	if err != nil || n != 1 {
		t.Fatalf("track: n=%d err=%v", n, err)
	}
	ttl := time.Hour
	if _, err := z.SendMessage(ctx, uid, DirectMessage{Title: "Hi {{custom.nick}}", TTL: &ttl}); err != nil {
		t.Fatalf("send message: %v", err)
	}
	inbox, err := z.Inbox(ctx, uid, 50)
	if err != nil {
		t.Fatalf("inbox: %v", err)
	}
	found := false
	for _, m := range inbox {
		if m.Title == "Hi Neo" && m.Key == nil && m.ExpiresAt != nil {
			found = true
		}
	}
	if !found {
		t.Fatalf("direct message not in inbox: %+v", inbox)
	}

	// Wait for the change stream (connecting clears the cache), then check caching behaviour.
	connected := false
	timeout := time.After(10 * time.Second)
	for !connected {
		select {
		case l := <-logs:
			connected = strings.Contains(l, "connected")
		case <-timeout:
			t.Fatal("change stream did not connect")
		}
	}
	flagsKey := "flags\x00" + uid + "\x00"
	if _, err := z.Flags(ctx, uid); err != nil {
		t.Fatal(err)
	}
	z.mu.Lock()
	_, cachedOnce := z.cache[flagsKey]
	size := len(z.cache)
	z.mu.Unlock()
	if !cachedOnce {
		t.Fatal("flags were not cached")
	}
	if _, err := z.Flags(ctx, uid); err != nil {
		t.Fatal(err)
	}
	z.mu.Lock()
	grew := len(z.cache) != size
	z.mu.Unlock()
	if grew {
		t.Fatal("repeated read must be served from cache")
	}
	if _, err := z.UpdateProperties(ctx, uid, nil, map[string]any{"nick": "Trinity"}); err != nil {
		t.Fatalf("update properties: %v", err)
	}
	z.mu.Lock()
	_, stillCached := z.cache[flagsKey]
	z.mu.Unlock()
	if stillCached {
		t.Fatal("UpdateProperties must invalidate the identity's cache")
	}
}
