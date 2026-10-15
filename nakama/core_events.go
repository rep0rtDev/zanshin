package main

import (
	"context"
	"database/sql"
	"os"
	"strconv"
	"time"

	"github.com/heroiclabs/nakama-common/api"
	"github.com/heroiclabs/nakama-common/runtime"
	zanshin "github.com/rep0rtDev/zanshin/sdk/go"
)

// Zero-code core events: Nakama's own purchase validation, leaderboard / tournament writes and
// friend invites are mirrored into Zanshin's core event catalog without any client changes.
// Disable with ZANSHIN_CORE_EVENTS=false.
func (m *module) registerCoreEvents(initializer runtime.Initializer) error {
	if v := os.Getenv("ZANSHIN_CORE_EVENTS"); v != "" {
		if on, err := strconv.ParseBool(v); err == nil && !on {
			m.logger.Info("zanshin: core event hooks disabled")
			return nil
		}
	}
	purchase := func(ctx context.Context, store string, out *api.ValidatePurchaseResponse) {
		if out == nil {
			return
		}
		uid, err := userID(ctx)
		if err != nil {
			return
		}
		events := make([]zanshin.Event, 0, len(out.ValidatedPurchases))
		for _, p := range out.ValidatedPurchases {
			if p == nil || p.SeenBefore {
				continue
			}
			meta := map[string]any{
				"store":          store,
				"product_id":     p.ProductId,
				"transaction_id": p.TransactionId,
				"environment":    p.Environment.String(),
				"source":         "nakama",
			}
			ev := zanshin.Event{Name: "purchaseCompleted", Value: p.ProductId, Metadata: meta}
			if p.PurchaseTime != nil {
				t := p.PurchaseTime.AsTime()
				ev.Timestamp = &t
			}
			events = append(events, ev)
		}
		m.track(uid, events...)
	}
	if err := initializer.RegisterAfterValidatePurchaseApple(func(ctx context.Context, logger runtime.Logger, db *sql.DB, nk runtime.NakamaModule, out *api.ValidatePurchaseResponse, in *api.ValidatePurchaseAppleRequest) error {
		purchase(ctx, "apple", out)
		return nil
	}); err != nil {
		return err
	}
	if err := initializer.RegisterAfterValidatePurchaseGoogle(func(ctx context.Context, logger runtime.Logger, db *sql.DB, nk runtime.NakamaModule, out *api.ValidatePurchaseResponse, in *api.ValidatePurchaseGoogleRequest) error {
		purchase(ctx, "google", out)
		return nil
	}); err != nil {
		return err
	}
	if err := initializer.RegisterAfterValidatePurchaseHuawei(func(ctx context.Context, logger runtime.Logger, db *sql.DB, nk runtime.NakamaModule, out *api.ValidatePurchaseResponse, in *api.ValidatePurchaseHuaweiRequest) error {
		purchase(ctx, "huawei", out)
		return nil
	}); err != nil {
		return err
	}
	if err := initializer.RegisterAfterValidatePurchaseFacebookInstant(func(ctx context.Context, logger runtime.Logger, db *sql.DB, nk runtime.NakamaModule, out *api.ValidatePurchaseResponse, in *api.ValidatePurchaseFacebookInstantRequest) error {
		purchase(ctx, "facebook_instant", out)
		return nil
	}); err != nil {
		return err
	}
	score := func(ctx context.Context, kind string, rec *api.LeaderboardRecord) {
		if rec == nil {
			return
		}
		uid, err := userID(ctx)
		if err != nil {
			return
		}
		m.track(uid, zanshin.Event{Name: "eventLeaderboardScore", Value: rec.LeaderboardId, Metadata: map[string]any{
			"leaderboard_id": rec.LeaderboardId, "kind": kind, "score": rec.Score, "subscore": rec.Subscore,
			"num_score": rec.NumScore, "rank": rec.Rank, "source": "nakama",
		}})
	}
	if err := initializer.RegisterAfterWriteLeaderboardRecord(func(ctx context.Context, logger runtime.Logger, db *sql.DB, nk runtime.NakamaModule, out *api.LeaderboardRecord, in *api.WriteLeaderboardRecordRequest) error {
		score(ctx, "leaderboard", out)
		return nil
	}); err != nil {
		return err
	}
	if err := initializer.RegisterAfterWriteTournamentRecord(func(ctx context.Context, logger runtime.Logger, db *sql.DB, nk runtime.NakamaModule, out *api.LeaderboardRecord, in *api.WriteTournamentRecordRequest) error {
		score(ctx, "tournament", out)
		return nil
	}); err != nil {
		return err
	}
	if err := initializer.RegisterAfterAddFriends(func(ctx context.Context, logger runtime.Logger, db *sql.DB, nk runtime.NakamaModule, in *api.AddFriendsRequest) error {
		if in == nil {
			return nil
		}
		uid, err := userID(ctx)
		if err != nil {
			return nil
		}
		n := len(in.Ids) + len(in.Usernames)
		if n > 0 {
			m.track(uid, zanshin.Event{Name: "inviteSent", Value: "friend", Metadata: map[string]any{"count": n, "channel": "nakama_friends", "source": "nakama"}})
		}
		return nil
	}); err != nil {
		return err
	}
	m.logger.Info("zanshin: core event hooks registered (purchases, leaderboards, tournaments, friend invites)")
	return nil
}

// track sends events in the background; hooks must never slow down or fail Nakama requests.
func (m *module) track(uid string, events ...zanshin.Event) {
	if len(events) == 0 {
		return
	}
	go func() {
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		if _, err := m.z.Track(ctx, uid, events...); err != nil {
			m.logger.Warn("zanshin: core event track failed for %s: %v", uid, err)
		}
	}()
}
