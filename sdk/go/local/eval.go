// Package local evaluates Zanshin feature flags in-process from a rules bundle
// (GET /server/v1/rules), bit-for-bit compatible with the server: same xxh3 bucketing,
// same condition operators, same reasons. Overrides, experiments and live events are
// server-side concerns and are not applied here.
package local

import (
	"encoding/json"
	"fmt"
	"math"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/zeebo/xxh3"
)

// Buckets is the server's bucket space.
const Buckets = 10000

// Bundle is the payload of GET /server/v1/rules.
type Bundle struct {
	GeneratedAt      time.Time  `json:"generated_at"`
	Flags            []Flag     `json:"flags"`
	Audiences        []Audience `json:"audiences"`
	ManagedAudiences []string   `json:"managed_audiences"`
}

type Flag struct {
	Key           string          `json:"key"`
	ValueType     string          `json:"value_type"`
	DefaultValue  json.RawMessage `json:"default_value"`
	Enabled       bool            `json:"enabled"`
	Rules         []Rule          `json:"rules"`
	Salt          string          `json:"salt"`
	Prerequisites []Prerequisite  `json:"prerequisites"`
	Version       int64           `json:"version"`
}

type Prerequisite struct {
	Flag  string          `json:"flag"`
	Value json.RawMessage `json:"value,omitempty"`
}

type Rule struct {
	ID         string          `json:"id"`
	Condition  Condition       `json:"condition"`
	Audiences  []string        `json:"audiences"`
	Percentage *float64        `json:"percentage"`
	Value      json.RawMessage `json:"value,omitempty"`
	Variants   []Variant       `json:"variants"`
}

type Variant struct {
	Key    string          `json:"key"`
	Value  json.RawMessage `json:"value"`
	Weight float64         `json:"weight"`
}

type Audience struct {
	Key        string    `json:"key"`
	Condition  Condition `json:"condition"`
	Percentage *float64  `json:"percentage"`
	Salt       string    `json:"salt"`
	Enabled    bool      `json:"enabled"`
}

// Condition mirrors the server's untagged enum: {all:[..]} | {any:[..]} | {not:{..}} | {prop,op,value}.
type Condition struct {
	All   []Condition `json:"all,omitempty"`
	Any   []Condition `json:"any,omitempty"`
	Not   *Condition  `json:"not,omitempty"`
	Prop  string      `json:"prop,omitempty"`
	Op    string      `json:"op,omitempty"`
	Value any         `json:"value,omitempty"`
	kind  string
}

func (c *Condition) UnmarshalJSON(b []byte) error {
	var raw map[string]json.RawMessage
	if err := json.Unmarshal(b, &raw); err != nil {
		return err
	}
	switch {
	case raw["all"] != nil:
		c.kind = "all"
		return json.Unmarshal(raw["all"], &c.All)
	case raw["any"] != nil:
		c.kind = "any"
		return json.Unmarshal(raw["any"], &c.Any)
	case raw["not"] != nil:
		c.kind = "not"
		c.Not = &Condition{}
		return json.Unmarshal(raw["not"], c.Not)
	default:
		c.kind = "cmp"
		type cmp struct {
			Prop  string `json:"prop"`
			Op    string `json:"op"`
			Value any    `json:"value"`
		}
		var x cmp
		if err := json.Unmarshal(b, &x); err != nil {
			return err
		}
		c.Prop, c.Op, c.Value = x.Prop, x.Op, x.Value
		return nil
	}
}

// Matches evaluates the condition against flattened properties ("default.country", "custom.level", …).
func (c *Condition) Matches(props map[string]any) bool {
	switch c.kind {
	case "all":
		for i := range c.All {
			if !c.All[i].Matches(props) {
				return false
			}
		}
		return true
	case "any":
		for i := range c.Any {
			if c.Any[i].Matches(props) {
				return true
			}
		}
		return false
	case "not":
		return !c.Not.Matches(props)
	}
	actual, ok := props[c.Prop]
	if actual == nil {
		ok = false
	}
	switch c.Op {
	case "exists":
		return ok
	case "not_exists":
		return !ok
	}
	if !ok {
		return c.Op == "neq" || c.Op == "not_in" || c.Op == "not_contains"
	}
	switch c.Op {
	case "eq":
		return looseEq(actual, c.Value)
	case "neq":
		return !looseEq(actual, c.Value)
	case "gt", "gte", "lt", "lte":
		o, ok := compare(actual, c.Value)
		if !ok {
			return false
		}
		switch c.Op {
		case "gt":
			return o > 0
		case "gte":
			return o >= 0
		case "lt":
			return o < 0
		default:
			return o <= 0
		}
	case "in", "not_in":
		arr, _ := c.Value.([]any)
		found := false
		for _, v := range arr {
			if looseEq(actual, v) {
				found = true
				break
			}
		}
		if c.Op == "in" {
			return found
		}
		return !found
	case "contains":
		return contains(actual, c.Value)
	case "not_contains":
		return !contains(actual, c.Value)
	case "starts_with", "ends_with", "matches":
		a, ok1 := actual.(string)
		b, ok2 := c.Value.(string)
		if !ok1 || !ok2 {
			return false
		}
		switch c.Op {
		case "starts_with":
			return strings.HasPrefix(a, b)
		case "ends_with":
			return strings.HasSuffix(a, b)
		default:
			re, err := regexp.Compile(b)
			return err == nil && re.MatchString(a)
		}
	case "semver_eq", "semver_gt", "semver_gte", "semver_lt", "semver_lte":
		a, ok1 := actual.(string)
		b, ok2 := c.Value.(string)
		if !ok1 || !ok2 {
			return false
		}
		o, ok := semverCmp(a, b)
		if !ok {
			return false
		}
		switch c.Op {
		case "semver_eq":
			return o == 0
		case "semver_gt":
			return o > 0
		case "semver_gte":
			return o >= 0
		case "semver_lt":
			return o < 0
		default:
			return o <= 0
		}
	case "within_last", "older_than":
		age, ok := ageSecs(actual)
		secs, ok2 := asFloat(c.Value)
		if !ok || !ok2 {
			return false
		}
		if c.Op == "within_last" {
			return age >= 0 && age <= secs
		}
		return age > secs
	}
	return false
}

func asFloat(v any) (float64, bool) {
	switch x := v.(type) {
	case float64:
		return x, true
	case int:
		return float64(x), true
	case int64:
		return float64(x), true
	case json.Number:
		f, err := x.Float64()
		return f, err == nil
	case string:
		f, err := strconv.ParseFloat(strings.TrimSpace(x), 64)
		return f, err == nil
	case bool:
		if x {
			return 1, true
		}
		return 0, true
	}
	return 0, false
}

func isNumber(v any) bool {
	switch v.(type) {
	case float64, int, int64, json.Number:
		return true
	}
	return false
}

func looseEq(a, b any) bool {
	if isNumber(a) && isNumber(b) {
		x, _ := asFloat(a)
		y, _ := asFloat(b)
		return x == y
	}
	switch x := a.(type) {
	case string:
		switch y := b.(type) {
		case string:
			return x == y
		case bool:
			p, err := strconv.ParseBool(x)
			return err == nil && p == y
		default:
			if isNumber(b) {
				f, err := strconv.ParseFloat(x, 64)
				n, _ := asFloat(b)
				return err == nil && f == n
			}
		}
	case bool:
		switch y := b.(type) {
		case bool:
			return x == y
		case string:
			p, err := strconv.ParseBool(y)
			return err == nil && p == x
		}
	default:
		if isNumber(a) {
			if s, ok := b.(string); ok {
				f, err := strconv.ParseFloat(s, 64)
				n, _ := asFloat(a)
				return err == nil && f == n
			}
		}
	}
	ja, _ := json.Marshal(a)
	jb, _ := json.Marshal(b)
	return string(ja) == string(jb) && !isNumber(a)
}

func compare(a, b any) (int, bool) {
	sa, aStr := a.(string)
	sb, bStr := b.(string)
	fa, aNum := asFloat(a)
	fb, bNum := asFloat(b)
	if aStr && bStr && (!aNum || !bNum) {
		return strings.Compare(sa, sb), true
	}
	if !aNum || !bNum {
		return 0, false
	}
	switch {
	case fa < fb:
		return -1, true
	case fa > fb:
		return 1, true
	}
	return 0, true
}

func contains(actual, needle any) bool {
	switch x := actual.(type) {
	case string:
		n, ok := needle.(string)
		return ok && strings.Contains(x, n)
	case []any:
		for _, v := range x {
			if looseEq(v, needle) {
				return true
			}
		}
	case map[string]any:
		n, ok := needle.(string)
		if !ok {
			return false
		}
		_, has := x[n]
		return has
	}
	return false
}

func parseSemver(s string) ([3]int64, string, bool) {
	s = strings.TrimSpace(s)
	s = strings.TrimLeft(s, "vV")
	pre := ""
	if i := strings.IndexAny(s, "-+"); i >= 0 {
		if s[i] == '-' {
			pre = s[i+1:]
			if j := strings.Index(pre, "+"); j >= 0 {
				pre = pre[:j]
			}
		}
		s = s[:i]
	}
	parts := strings.Split(s, ".")
	for len(parts) < 3 {
		parts = append(parts, "0")
	}
	if len(parts) > 3 {
		return [3]int64{}, "", false
	}
	var out [3]int64
	for i, p := range parts {
		n, err := strconv.ParseInt(p, 10, 64)
		if err != nil || n < 0 {
			return [3]int64{}, "", false
		}
		out[i] = n
	}
	return out, pre, true
}

func semverCmp(a, b string) (int, bool) {
	va, pa, ok1 := parseSemver(a)
	vb, pb, ok2 := parseSemver(b)
	if !ok1 || !ok2 {
		return 0, false
	}
	for i := 0; i < 3; i++ {
		if va[i] != vb[i] {
			if va[i] < vb[i] {
				return -1, true
			}
			return 1, true
		}
	}
	// a pre-release sorts before the release
	switch {
	case pa == pb:
		return 0, true
	case pa == "":
		return 1, true
	case pb == "":
		return -1, true
	}
	return strings.Compare(pa, pb), true
}

func ageSecs(v any) (float64, bool) {
	var ts time.Time
	switch x := v.(type) {
	case string:
		t, err := time.Parse(time.RFC3339Nano, x)
		if err != nil {
			return 0, false
		}
		ts = t
	default:
		f, ok := asFloat(v)
		if !ok {
			return 0, false
		}
		if _, isStr := v.(string); isStr {
			return 0, false
		}
		if f > 1e11 {
			f /= 1000
		}
		ts = time.Unix(int64(f), 0)
	}
	return time.Since(ts).Seconds(), true
}

// Bucket is the server's hash bucket: xxh3_64(namespace \0 salt \0 subject) % 10000.
func Bucket(namespace, salt, subject string) uint32 {
	b := make([]byte, 0, len(namespace)+len(salt)+len(subject)+2)
	b = append(b, namespace...)
	b = append(b, 0)
	b = append(b, salt...)
	b = append(b, 0)
	b = append(b, subject...)
	return uint32(xxh3.Hash(b) % Buckets)
}

func InRollout(namespace, salt, subject string, percentage float64) bool {
	if percentage >= 100 {
		return true
	}
	if percentage <= 0 {
		return false
	}
	threshold := uint32(math.Round(percentage / 100 * Buckets))
	return Bucket(namespace, salt, subject) < threshold
}

// WeightedPick returns the index chosen for subject, or -1 when no weight is positive.
func WeightedPick(namespace, salt, subject string, weights []float64) int {
	total := 0.0
	for _, w := range weights {
		if w > 0 {
			total += w
		}
	}
	if total <= 0 {
		return -1
	}
	point := float64(Bucket(namespace, salt, subject)) / Buckets * total
	acc := 0.0
	for i, w := range weights {
		if w <= 0 {
			continue
		}
		acc += w
		if point < acc {
			return i
		}
	}
	for i := len(weights) - 1; i >= 0; i-- {
		if weights[i] > 0 {
			return i
		}
	}
	return -1
}

// Reason mirrors the server's `reason` object.
type Reason struct {
	Kind    string `json:"kind"`
	RuleID  string `json:"rule_id,omitempty"`
	Variant string `json:"variant,omitempty"`
	Flag    string `json:"flag,omitempty"`
}

// Result is one evaluated flag.
type Result struct {
	Key       string          `json:"key"`
	Value     json.RawMessage `json:"value"`
	ValueType string          `json:"value_type"`
	Variant   string          `json:"variant,omitempty"`
	Reason    Reason          `json:"reason"`
	Version   int64           `json:"version"`
}

// Evaluator holds an indexed bundle.
type Evaluator struct {
	flags     map[string]*Flag
	audiences map[string]*Audience
	managed   map[string]bool
	bundle    *Bundle
}

func New(b *Bundle) *Evaluator {
	e := &Evaluator{flags: map[string]*Flag{}, audiences: map[string]*Audience{}, managed: map[string]bool{}, bundle: b}
	for i := range b.Flags {
		e.flags[b.Flags[i].Key] = &b.Flags[i]
	}
	for i := range b.Audiences {
		e.audiences[b.Audiences[i].Key] = &b.Audiences[i]
	}
	for _, k := range b.ManagedAudiences {
		e.managed[k] = true
	}
	return e
}

// Bundle returns the bundle this evaluator was built from.
func (e *Evaluator) Bundle() *Bundle { return e.bundle }

// Flatten turns a property set {default:{..}, computed:{..}, custom:{..}} into "ns.key" entries
// and adds identity.external_id.
func Flatten(externalID string, props map[string]map[string]any) map[string]any {
	flat := map[string]any{}
	for _, ns := range []string{"default", "computed", "custom"} {
		for k, v := range props[ns] {
			flat[ns+"."+k] = v
		}
	}
	flat["identity.external_id"] = externalID
	return flat
}

// InAudience reports dynamic-audience membership; managed audiences are never matched locally.
func (e *Evaluator) InAudience(key, subject string, flat map[string]any) bool {
	a, ok := e.audiences[key]
	if !ok || !a.Enabled || !a.Condition.Matches(flat) {
		return false
	}
	pct := 100.0
	if a.Percentage != nil {
		pct = *a.Percentage
	}
	return InRollout("audience:"+key, a.Salt, subject, pct)
}

// DependsOnManaged reports whether any rule of the flag references a managed audience, i.e. the
// local result may differ from the server's and a remote evaluation is advisable.
func (e *Evaluator) DependsOnManaged(key string) bool {
	f, ok := e.flags[key]
	if !ok {
		return false
	}
	for _, r := range f.Rules {
		for _, a := range r.Audiences {
			if e.managed[a] {
				return true
			}
		}
	}
	return false
}

func (e *Evaluator) evaluateOne(f *Flag, subject string, flat map[string]any) Result {
	res := Result{Key: f.Key, Value: f.DefaultValue, ValueType: f.ValueType, Reason: Reason{Kind: "default"}, Version: f.Version}
	if !f.Enabled {
		res.Reason = Reason{Kind: "disabled"}
		return res
	}
	ns := "flag:" + f.Key
	for _, r := range f.Rules {
		if !r.Condition.Matches(flat) {
			continue
		}
		member := true
		for _, a := range r.Audiences {
			if !e.InAudience(a, subject, flat) {
				member = false
				break
			}
		}
		if !member {
			continue
		}
		pct := 100.0
		if r.Percentage != nil {
			pct = *r.Percentage
		}
		if !InRollout(ns, f.Salt+":"+r.ID, subject, pct) {
			continue
		}
		if len(r.Value) > 0 && string(r.Value) != "null" {
			res.Value = r.Value
			res.Reason = Reason{Kind: "rule", RuleID: r.ID}
			return res
		}
		weights := make([]float64, len(r.Variants))
		for i, v := range r.Variants {
			weights[i] = v.Weight
		}
		if i := WeightedPick(ns, f.Salt+":"+r.ID+":variant", subject, weights); i >= 0 {
			v := r.Variants[i]
			res.Value = v.Value
			res.Variant = v.Key
			res.Reason = Reason{Kind: "variant", RuleID: r.ID, Variant: v.Key}
			return res
		}
	}
	return res
}

// Evaluate resolves one flag; ok is false when the flag is unknown.
func (e *Evaluator) Evaluate(key, subject string, flat map[string]any) (Result, bool) {
	f, ok := e.flags[key]
	if !ok {
		return Result{}, false
	}
	res := e.evaluateOne(f, subject, flat)
	if len(f.Prerequisites) > 0 {
		for _, p := range f.Prerequisites {
			dep, ok := e.flags[p.Flag]
			if !ok {
				return e.prereqDefault(res, f, p.Flag), true
			}
			d := e.evaluateOne(dep, subject, flat)
			if d.Reason.Kind == "disabled" || d.Reason.Kind == "prerequisite" {
				return e.prereqDefault(res, f, p.Flag), true
			}
			if len(p.Value) > 0 && string(p.Value) != "null" && !jsonEq(p.Value, d.Value) {
				return e.prereqDefault(res, f, p.Flag), true
			}
		}
	}
	return res, true
}

func (e *Evaluator) prereqDefault(res Result, f *Flag, dep string) Result {
	res.Value = f.DefaultValue
	res.Variant = ""
	res.Reason = Reason{Kind: "prerequisite", Flag: dep}
	return res
}

func jsonEq(a, b json.RawMessage) bool {
	var x, y any
	if json.Unmarshal(a, &x) != nil || json.Unmarshal(b, &y) != nil {
		return false
	}
	ja, _ := json.Marshal(x)
	jb, _ := json.Marshal(y)
	return string(ja) == string(jb)
}

// EvaluateAll resolves every flag (or only names), sorted by key like the server.
func (e *Evaluator) EvaluateAll(subject string, flat map[string]any, names ...string) []Result {
	keys := make([]string, 0, len(e.flags))
	if len(names) == 0 {
		for k := range e.flags {
			keys = append(keys, k)
		}
	} else {
		for _, n := range names {
			if _, ok := e.flags[n]; ok {
				keys = append(keys, n)
			}
		}
	}
	sort.Strings(keys)
	out := make([]Result, 0, len(keys))
	for _, k := range keys {
		r, _ := e.Evaluate(k, subject, flat)
		out = append(out, r)
	}
	return out
}

// String renders a Result for logs.
func (r Result) String() string {
	return fmt.Sprintf("%s=%s (%s)", r.Key, string(r.Value), r.Reason.Kind)
}
