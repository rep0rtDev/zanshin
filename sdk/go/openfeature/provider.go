// Package openfeature is an OpenFeature provider backed by Zanshin.
//
//	z := zanshin.New(url, serverKey, zanshin.WithLocalEvaluation(time.Minute))
//	openfeature.SetProvider(zanshinof.New(z, zanshinof.Local()))
//	client := openfeature.NewClient("game")
//	on, _ := client.BooleanValue(ctx, "shop", false, openfeature.NewEvaluationContext("player-42", map[string]any{"level": 7}))
//
// The targeting key is the player's external id. In local mode the other context attributes
// become `custom.*` properties (nested "default" / "custom" maps are honoured); in remote mode
// the identity's stored properties are used.
package openfeature

import (
	"context"
	"encoding/json"
	"time"

	of "github.com/open-feature/go-sdk/openfeature"
	zanshin "github.com/rep0rtDev/zanshin/sdk/go"
)

type Option func(*Provider)

// Local evaluates flags in-process from the rules bundle instead of calling /server/v1/flags.
func Local() Option { return func(p *Provider) { p.local = true } }

// Timeout bounds each remote call (default 3s).
func Timeout(d time.Duration) Option { return func(p *Provider) { p.timeout = d } }

type Provider struct {
	z       *zanshin.Client
	local   bool
	timeout time.Duration
}

func New(z *zanshin.Client, opts ...Option) *Provider {
	p := &Provider{z: z, timeout: 3 * time.Second}
	for _, o := range opts {
		o(p)
	}
	return p
}

func (p *Provider) Metadata() of.Metadata { return of.Metadata{Name: "zanshin"} }
func (p *Provider) Hooks() []of.Hook      { return nil }

func props(evalCtx of.FlattenedContext) map[string]map[string]any {
	out := map[string]map[string]any{"custom": {}}
	for k, v := range evalCtx {
		if k == of.TargetingKey {
			continue
		}
		if m, ok := v.(map[string]any); ok && (k == "default" || k == "computed" || k == "custom") {
			if out[k] == nil {
				out[k] = map[string]any{}
			}
			for kk, vv := range m {
				out[k][kk] = vv
			}
			continue
		}
		out["custom"][k] = v
	}
	return out
}

func (p *Provider) resolve(ctx context.Context, flag string, evalCtx of.FlattenedContext) (*zanshin.Flag, of.ProviderResolutionDetail) {
	subject, _ := evalCtx[of.TargetingKey].(string)
	if p.timeout > 0 {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, p.timeout)
		defer cancel()
	}
	var f *zanshin.Flag
	var err error
	if p.local {
		f, err = p.z.LocalFlag(ctx, subject, flag, props(evalCtx))
	} else {
		if subject == "" {
			return nil, of.ProviderResolutionDetail{ResolutionError: of.NewTargetingKeyMissingResolutionError("targetingKey (player external id) is required"), Reason: of.ErrorReason}
		}
		f, err = p.z.Flag(ctx, subject, flag)
	}
	if err != nil {
		if e, ok := err.(*zanshin.Error); ok && e.Status == 404 {
			return nil, of.ProviderResolutionDetail{ResolutionError: of.NewFlagNotFoundResolutionError(err.Error()), Reason: of.ErrorReason}
		}
		return nil, of.ProviderResolutionDetail{ResolutionError: of.NewGeneralResolutionError(err.Error()), Reason: of.ErrorReason}
	}
	d := of.ProviderResolutionDetail{Reason: reason(f.Reason.Kind), Variant: f.Variant, FlagMetadata: of.FlagMetadata{"version": f.Version, "reason": f.Reason.Kind}}
	if f.Reason.RuleID != "" {
		d.FlagMetadata["rule_id"] = f.Reason.RuleID
	}
	return f, d
}

func reason(kind string) of.Reason {
	switch kind {
	case "default", "prerequisite":
		return of.DefaultReason
	case "disabled":
		return of.DisabledReason
	case "variant", "experiment":
		return of.SplitReason
	case "rule", "override", "live_event":
		return of.TargetingMatchReason
	}
	return of.UnknownReason
}

func mismatch(d of.ProviderResolutionDetail, want string) of.ProviderResolutionDetail {
	d.ResolutionError = of.NewTypeMismatchResolutionError("flag value is not a " + want)
	d.Reason = of.ErrorReason
	return d
}

func (p *Provider) BooleanEvaluation(ctx context.Context, flag string, def bool, evalCtx of.FlattenedContext) of.BoolResolutionDetail {
	f, d := p.resolve(ctx, flag, evalCtx)
	if f == nil {
		return of.BoolResolutionDetail{Value: def, ProviderResolutionDetail: d}
	}
	var v bool
	if json.Unmarshal(f.Value, &v) != nil {
		return of.BoolResolutionDetail{Value: def, ProviderResolutionDetail: mismatch(d, "bool")}
	}
	return of.BoolResolutionDetail{Value: v, ProviderResolutionDetail: d}
}

func (p *Provider) StringEvaluation(ctx context.Context, flag string, def string, evalCtx of.FlattenedContext) of.StringResolutionDetail {
	f, d := p.resolve(ctx, flag, evalCtx)
	if f == nil {
		return of.StringResolutionDetail{Value: def, ProviderResolutionDetail: d}
	}
	var v string
	if json.Unmarshal(f.Value, &v) != nil {
		return of.StringResolutionDetail{Value: def, ProviderResolutionDetail: mismatch(d, "string")}
	}
	return of.StringResolutionDetail{Value: v, ProviderResolutionDetail: d}
}

func (p *Provider) FloatEvaluation(ctx context.Context, flag string, def float64, evalCtx of.FlattenedContext) of.FloatResolutionDetail {
	f, d := p.resolve(ctx, flag, evalCtx)
	if f == nil {
		return of.FloatResolutionDetail{Value: def, ProviderResolutionDetail: d}
	}
	var v float64
	if json.Unmarshal(f.Value, &v) != nil {
		return of.FloatResolutionDetail{Value: def, ProviderResolutionDetail: mismatch(d, "number")}
	}
	return of.FloatResolutionDetail{Value: v, ProviderResolutionDetail: d}
}

func (p *Provider) IntEvaluation(ctx context.Context, flag string, def int64, evalCtx of.FlattenedContext) of.IntResolutionDetail {
	f, d := p.resolve(ctx, flag, evalCtx)
	if f == nil {
		return of.IntResolutionDetail{Value: def, ProviderResolutionDetail: d}
	}
	var v float64
	if json.Unmarshal(f.Value, &v) != nil || v != float64(int64(v)) {
		return of.IntResolutionDetail{Value: def, ProviderResolutionDetail: mismatch(d, "integer")}
	}
	return of.IntResolutionDetail{Value: int64(v), ProviderResolutionDetail: d}
}

func (p *Provider) ObjectEvaluation(ctx context.Context, flag string, def any, evalCtx of.FlattenedContext) of.InterfaceResolutionDetail {
	f, d := p.resolve(ctx, flag, evalCtx)
	if f == nil {
		return of.InterfaceResolutionDetail{Value: def, ProviderResolutionDetail: d}
	}
	var v any
	if json.Unmarshal(f.Value, &v) != nil {
		return of.InterfaceResolutionDetail{Value: def, ProviderResolutionDetail: mismatch(d, "json value")}
	}
	return of.InterfaceResolutionDetail{Value: v, ProviderResolutionDetail: d}
}

var _ of.FeatureProvider = (*Provider)(nil)
