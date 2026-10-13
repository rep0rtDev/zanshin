package zanshin

import (
	"context"
	"encoding/json"
	"net/http"
	"strings"
	"time"

	"github.com/coder/websocket"
)

// StartStream follows the server change feed in the background and drops cached evaluations
// whenever flags, audiences, experiments, live events or messages change. It reconnects with
// backoff until Close is called or ctx ends. Safe to call once per client.
func (c *Client) StartStream(ctx context.Context) {
	c.mu.Lock()
	if c.streamCancel != nil {
		c.mu.Unlock()
		return
	}
	ctx, cancel := context.WithCancel(ctx)
	c.streamCancel = cancel
	c.mu.Unlock()

	go func() {
		backoff := time.Second
		for ctx.Err() == nil {
			err := c.runStream(ctx)
			if ctx.Err() != nil {
				return
			}
			c.logf("zanshin: change stream disconnected: %v (retry in %s)", err, backoff)
			select {
			case <-ctx.Done():
				return
			case <-time.After(backoff):
			}
			if backoff < 30*time.Second {
				backoff *= 2
			}
		}
	}()
}

func (c *Client) wsURL() string {
	u := c.baseURL
	switch {
	case strings.HasPrefix(u, "https://"):
		u = "wss://" + strings.TrimPrefix(u, "https://")
	case strings.HasPrefix(u, "http://"):
		u = "ws://" + strings.TrimPrefix(u, "http://")
	}
	return u + "/server/v1/stream"
}

func (c *Client) runStream(ctx context.Context) error {
	dialCtx, cancel := context.WithTimeout(ctx, 10*time.Second)
	conn, _, err := websocket.Dial(dialCtx, c.wsURL(), &websocket.DialOptions{
		HTTPHeader: http.Header{"x-api-key": {c.apiKey}},
		HTTPClient: c.http,
	})
	cancel()
	if err != nil {
		return err
	}
	defer conn.Close(websocket.StatusNormalClosure, "bye")
	// Anything may have changed while we were away.
	c.Invalidate()
	c.logf("zanshin: change stream connected")

	pingCtx, stopPing := context.WithCancel(ctx)
	defer stopPing()
	go func() {
		t := time.NewTicker(30 * time.Second)
		defer t.Stop()
		for {
			select {
			case <-pingCtx.Done():
				return
			case <-t.C:
				_ = conn.Write(pingCtx, websocket.MessageText, []byte(`{"type":"ping"}`))
			}
		}
	}()

	for {
		_, data, err := conn.Read(ctx)
		if err != nil {
			return err
		}
		var ch Change
		if json.Unmarshal(data, &ch) != nil {
			continue
		}
		switch ch.Type {
		case "change", "resync":
			// Identity-scoped changes carry an internal id we cannot map to an external id
			// cheaply, so every change clears the whole cache; evaluations are cheap to refetch.
			c.Invalidate()
		}
	}
}
