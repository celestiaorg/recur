package usage

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"
)

const defaultHTTPTimeout = 15 * time.Second

func defaultClient() *http.Client {
	return &http.Client{Timeout: defaultHTTPTimeout}
}

func doJSON(ctx context.Context, client *http.Client, method, url string, body io.Reader, headers map[string]string) (int, []byte, error) {
	if client == nil {
		client = defaultClient()
	}
	req, err := http.NewRequestWithContext(ctx, method, url, body)
	if err != nil {
		return 0, nil, err
	}
	for k, v := range headers {
		if v != "" {
			req.Header.Set(k, v)
		}
	}
	resp, err := client.Do(req)
	if err != nil {
		return 0, nil, err
	}
	defer resp.Body.Close()
	raw, err := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
	if err != nil {
		return resp.StatusCode, nil, err
	}
	return resp.StatusCode, raw, nil
}

func httpErr(status int, raw []byte) error {
	msg := strings.TrimSpace(string(raw))
	if len(msg) > 180 {
		msg = msg[:180]
	}
	if msg == "" {
		return fmt.Errorf("HTTP %d", status)
	}
	return fmt.Errorf("HTTP %d: %s", status, msg)
}

func isRateLimited(status int) bool {
	return status == http.StatusTooManyRequests
}

func isAuthErr(status int) bool {
	return status == http.StatusUnauthorized || status == http.StatusForbidden
}

func isReachabilityError(msg string) bool {
	return strings.HasPrefix(msg, "Could not reach")
}
