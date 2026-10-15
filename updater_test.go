package main

import (
	"context"
	"io"
	"testing"

	"github.com/wailsapp/wails/v3/pkg/updater"
)

type fakeProvider struct{ rel *updater.Release }

func (fakeProvider) Name() string { return "fake" }
func (f fakeProvider) Check(context.Context, updater.CheckRequest) (*updater.Release, error) {
	return f.rel, nil
}
func (fakeProvider) Download(context.Context, *updater.Release, io.Writer, func(int64, int64)) error {
	return nil
}

func TestSignedOnlyRejectsUnsignedReleases(t *testing.T) {
	for name, tc := range map[string]struct {
		rel     *updater.Release
		wantErr bool
	}{
		"up to date":  {nil, false},
		"no verify":   {&updater.Release{Version: "2.0.0"}, true},
		"digest only": {&updater.Release{Version: "2.0.0", Verification: &updater.Verification{Digest: []byte{1}}}, true},
		"signed":      {&updater.Release{Version: "2.0.0", Verification: &updater.Verification{Signature: []byte{1}}}, false},
	} {
		rel, err := signedOnly{fakeProvider{tc.rel}}.Check(context.Background(), updater.CheckRequest{})
		if (err != nil) != tc.wantErr {
			t.Errorf("%s: err = %v, wantErr %v", name, err, tc.wantErr)
		}
		if err == nil && rel != tc.rel {
			t.Errorf("%s: release not passed through", name)
		}
	}
}
