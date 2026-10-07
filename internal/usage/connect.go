package usage

import "context"

func (f *Fetcher) StartConnect(onDone func(error)) CursorUsageConnect {
	f.mu.Lock()
	if f.connecting && f.connectURL != "" {
		url := f.connectURL
		f.mu.Unlock()
		return CursorUsageConnect{LoginURL: url, Pending: true}
	}
	login, err := generateCursorPKCE(f.CursorLoginURL)
	if err != nil {
		f.mu.Unlock()
		return CursorUsageConnect{Error: err.Error()}
	}
	ctx, cancel := context.WithTimeout(context.Background(), cursorConnectTimeout)
	f.connecting = true
	f.connectCancel = cancel
	f.connectURL = login.LoginURL
	f.mu.Unlock()

	go func() {
		defer cancel()
		tok, err := pollCursorLogin(ctx, f, login)
		f.mu.Lock()
		f.connecting = false
		f.connectCancel = nil
		f.connectURL = ""
		f.mu.Unlock()
		if err != nil {
			if onDone != nil {
				onDone(err)
			}
			return
		}
		if dir := f.dataDir(); dir != "" {
			if tok.Email == "" {
				tok.Email = fetchCursorEmail(context.Background(), f, tok.Access)
			}
			tok = f.ensureCursorAPIKey(context.Background(), tok)
			_ = writeCursorAuthFile(cursorAuthPath(dir), tok)
		}
		if onDone != nil {
			onDone(nil)
		}
	}()
	return CursorUsageConnect{LoginURL: login.LoginURL, Pending: true}
}

func (f *Fetcher) CancelConnect() {
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.connectCancel != nil {
		f.connectCancel()
		f.connectCancel = nil
	}
	f.connecting = false
	f.connectURL = ""
}

func cursorHintNoSession() string {
	return "Connect in Settings."
}

// CursorNoSessionQuota is a local-only row so Connect stays visible without a fetch.
func CursorNoSessionQuota() ProviderQuota {
	return ProviderQuota{
		ID: ProviderCursor, Name: "Cursor",
		Error: "No Cursor usage session",
		Hint:  cursorHintNoSession(),
	}
}

// CursorConnectTokensQuota is a local-only row so Disconnect stays reachable
// when Cursor is off but Recur still owns Connect tokens.
func CursorConnectTokensQuota() ProviderQuota {
	q := ProviderQuota{
		ID: ProviderCursor, Name: "Cursor",
		Hint: "Cursor is off in Settings — tokens kept so you can disconnect.",
	}
	setSource(&q, SourceCursorConnect)
	return q
}

// CursorConnectingQuota is the in-flight Connect row (no network).
func CursorConnectingQuota() ProviderQuota {
	return ProviderQuota{
		ID: ProviderCursor, Name: "Cursor",
		Connecting: true, Hint: "Waiting for browser sign-in…",
	}
}
