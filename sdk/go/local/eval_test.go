package local

import (
	"encoding/json"
	"strings"
	"testing"
)

// vectors produced by the server's zanshin_core::hash::bucket
func TestBucketVectors(t *testing.T) {
	cases := []struct {
		ns, salt, subj string
		want           uint32
	}{
		{"flag:a", "s1", "user-1", 1931},
		{"audience:vip", "", "00000000-0000-0000-0000-000000000001", 6244},
		{"flag:ünïcode", "sålt", "пользователь-42", 1054},
		{"flag:long", "salt", strings.Repeat("x", 300), 9691},
		{"", "", "", 213},
	}
	for _, c := range cases {
		if got := Bucket(c.ns, c.salt, c.subj); got != c.want {
			t.Errorf("bucket(%q,%q,…) = %d, want %d", c.ns, c.salt, got, c.want)
		}
	}
	if i := WeightedPick("flag:a", "s1:r1:variant", "user-1", []float64{1, 1, 2}); i != 0 {
		t.Errorf("weighted_pick = %d, want 0", i)
	}
	if InRollout("flag:a", "s1:r1", "user-1", 50) {
		t.Errorf("in_rollout(50%%) should be false for bucket 1931? got true")
	}
}

func TestEvaluate(t *testing.T) {
	var b Bundle
	if err := json.Unmarshal([]byte(`{
	  "generated_at": "2026-01-01T00:00:00Z",
	  "flags": [
	    {"key":"shop","value_type":"bool","default_value":false,"enabled":true,"salt":"s","version":3,
	     "rules":[
	       {"id":"vip","condition":{"all":[]},"audiences":["vip"],"percentage":100,"value":true},
	       {"id":"de","condition":{"prop":"default.country","op":"in","value":["DE","AT"]},"percentage":100,
	        "variants":[{"key":"a","value":true,"weight":1},{"key":"b","value":false,"weight":1}]}
	     ]},
	    {"key":"off","value_type":"string","default_value":"x","enabled":false,"salt":"s","version":1,"rules":[]},
	    {"key":"child","value_type":"number","default_value":1,"enabled":true,"salt":"s","version":1,
	     "prerequisites":[{"flag":"shop","value":true}],
	     "rules":[{"id":"r","condition":{"prop":"custom.level","op":"gte","value":"10"},"percentage":100,"value":2}]}
	  ],
	  "audiences": [{"key":"vip","condition":{"prop":"custom.tags","op":"contains","value":"vip"},"percentage":100,"salt":"","enabled":true}],
	  "managed_audiences": ["whales"]
	}`), &b); err != nil {
		t.Fatal(err)
	}
	e := New(&b)
	flat := Flatten("u1", map[string]map[string]any{"default": {"country": "FR"}, "custom": {"tags": []any{"vip"}, "level": 12.0}})
	r, ok := e.Evaluate("shop", "u1", flat)
	if !ok || string(r.Value) != "true" || r.Reason.Kind != "rule" || r.Reason.RuleID != "vip" {
		t.Fatalf("vip rule: %+v", r)
	}
	flat2 := Flatten("u1", map[string]map[string]any{"default": {"country": "DE"}})
	r, _ = e.Evaluate("shop", "u1", flat2)
	if r.Reason.Kind != "variant" || r.Reason.RuleID != "de" || r.Variant == "" {
		t.Fatalf("variant rule: %+v", r)
	}
	r, _ = e.Evaluate("shop", "u1", Flatten("u1", nil))
	if r.Reason.Kind != "default" || string(r.Value) != "false" {
		t.Fatalf("default: %+v", r)
	}
	r, _ = e.Evaluate("off", "u1", flat)
	if r.Reason.Kind != "disabled" {
		t.Fatalf("disabled: %+v", r)
	}
	r, _ = e.Evaluate("child", "u1", flat)
	if r.Reason.Kind != "rule" || string(r.Value) != "2" {
		t.Fatalf("child with met prerequisite: %+v", r)
	}
	r, _ = e.Evaluate("child", "u1", Flatten("u1", map[string]map[string]any{"custom": {"level": 12.0}}))
	if r.Reason.Kind != "prerequisite" || r.Reason.Flag != "shop" || string(r.Value) != "1" {
		t.Fatalf("child with unmet prerequisite: %+v", r)
	}
	if _, ok := e.Evaluate("nope", "u1", flat); ok {
		t.Fatal("unknown flag must not resolve")
	}
	all := e.EvaluateAll("u1", flat)
	if len(all) != 3 || all[0].Key != "child" || all[2].Key != "shop" {
		t.Fatalf("sorted all: %+v", all)
	}
	if e.DependsOnManaged("shop") {
		t.Fatal("shop does not reference a managed audience")
	}
}

func TestConditions(t *testing.T) {
	flat := map[string]any{"default.country": "DE", "default.app_version": "1.4.2", "custom.level": 12.0, "custom.tags": []any{"beta", "vip"}, "custom.beta": "true", "default.created_at": "2020-01-01T00:00:00Z"}
	ok := func(j string) bool {
		var c Condition
		if err := json.Unmarshal([]byte(j), &c); err != nil {
			t.Fatal(err)
		}
		return c.Matches(flat)
	}
	for _, c := range []string{
		`{"prop":"default.country","op":"eq","value":"DE"}`,
		`{"prop":"default.country","op":"in","value":["AT","DE"]}`,
		`{"prop":"custom.level","op":"gte","value":12}`,
		`{"prop":"custom.level","op":"lt","value":"20"}`,
		`{"prop":"custom.tags","op":"contains","value":"vip"}`,
		`{"prop":"custom.beta","op":"eq","value":true}`,
		`{"prop":"default.app_version","op":"semver_gte","value":"v1.4"}`,
		`{"prop":"default.app_version","op":"semver_lt","value":"2.0.0"}`,
		`{"prop":"default.country","op":"matches","value":"^D"}`,
		`{"prop":"missing","op":"not_exists"}`,
		`{"prop":"missing","op":"neq","value":1}`,
		`{"prop":"default.created_at","op":"older_than","value":86400}`,
		`{"not":{"prop":"default.country","op":"eq","value":"FR"}}`,
		`{"any":[{"prop":"default.country","op":"eq","value":"FR"},{"prop":"custom.level","op":"gt","value":10}]}`,
	} {
		if !ok(c) {
			t.Errorf("expected match: %s", c)
		}
	}
	for _, c := range []string{
		`{"prop":"default.country","op":"not_in","value":["AT","DE"]}`,
		`{"prop":"custom.level","op":"gt","value":12}`,
		`{"prop":"missing","op":"eq","value":1}`,
		`{"prop":"default.created_at","op":"within_last","value":60}`,
	} {
		if ok(c) {
			t.Errorf("expected no match: %s", c)
		}
	}
}
