package openfeature

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	of "github.com/open-feature/go-sdk/openfeature"
	zanshin "github.com/rep0rtDev/zanshin/sdk/go"
)

func server(t *testing.T) *httptest.Server {
	mux := http.NewServeMux()
	mux.HandleFunc("/server/v1/rules", func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("x-api-key") != "k" {
			w.WriteHeader(401)
			return
		}
		w.Header().Set("content-type", "application/json")
		_, _ = w.Write([]byte(`{"generated_at":"2026-01-01T00:00:00Z","managed_audiences":[],"audiences":[],"flags":[
		  {"key":"shop","value_type":"bool","default_value":false,"enabled":true,"salt":"s","version":1,
		   "rules":[{"id":"r","condition":{"prop":"custom.level","op":"gte","value":10},"percentage":100,"value":true}]},
		  {"key":"title","value_type":"string","default_value":"hi","enabled":true,"salt":"s","version":1,"rules":[]},
		  {"key":"limit","value_type":"number","default_value":5,"enabled":true,"salt":"s","version":1,"rules":[]}]}`))
	})
	mux.HandleFunc("/server/v1/flags", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("content-type", "application/json")
		_, _ = w.Write([]byte(`{"flags":[{"key":"shop","value":true,"value_type":"bool","reason":{"kind":"override"},"version":2}]}`))
	})
	s := httptest.NewServer(mux)
	t.Cleanup(s.Close)
	return s
}

func TestLocalProvider(t *testing.T) {
	s := server(t)
	z := zanshin.New(s.URL, "k", zanshin.WithLocalEvaluation(time.Minute))
	p := New(z, Local())
	ctx := context.Background()
	d := p.BooleanEvaluation(ctx, "shop", false, of.FlattenedContext{of.TargetingKey: "u1", "level": 12.0})
	if !d.Value || d.Reason != of.TargetingMatchReason || d.FlagMetadata["rule_id"] != "r" {
		t.Fatalf("targeting: %+v", d)
	}
	d = p.BooleanEvaluation(ctx, "shop", false, of.FlattenedContext{of.TargetingKey: "u1", "custom": map[string]any{"level": 3.0}})
	if d.Value || d.Reason != of.DefaultReason {
		t.Fatalf("default: %+v", d)
	}
	sd := p.StringEvaluation(ctx, "shop", "x", of.FlattenedContext{of.TargetingKey: "u1"})
	if sd.Value != "x" || sd.ResolutionError.Error() == "" || sd.Reason != of.ErrorReason {
		t.Fatalf("type mismatch: %+v", sd)
	}
	if v := p.StringEvaluation(ctx, "title", "x", of.FlattenedContext{}).Value; v != "hi" {
		t.Fatalf("title = %q", v)
	}
	if v := p.IntEvaluation(ctx, "limit", 1, of.FlattenedContext{}).Value; v != 5 {
		t.Fatalf("limit = %d", v)
	}
	nf := p.FloatEvaluation(ctx, "nope", 1.5, of.FlattenedContext{})
	if nf.Value != 1.5 || nf.ResolutionError.Error() == "" {
		t.Fatalf("not found: %+v", nf)
	}
}

func TestRemoteProvider(t *testing.T) {
	s := server(t)
	p := New(zanshin.New(s.URL, "k"))
	ctx := context.Background()
	d := p.BooleanEvaluation(ctx, "shop", false, of.FlattenedContext{of.TargetingKey: "u1"})
	if !d.Value || d.Reason != of.TargetingMatchReason {
		t.Fatalf("remote: %+v", d)
	}
	if e := p.BooleanEvaluation(ctx, "shop", false, of.FlattenedContext{}); e.ResolutionError.Error() == "" {
		t.Fatalf("missing targeting key must error: %+v", e)
	}
}
