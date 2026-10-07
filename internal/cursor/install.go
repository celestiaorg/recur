package cursor

import (
	"archive/zip"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"path"
	"path/filepath"
	"runtime"
	"strings"
	"time"
)

// Installing the bridge means fetching Cursor's `cursor-sdk` Python wheel and
// unpacking the `cursor_sdk/_vendor/bridge` directory out of it — the bridge is
// only distributed inside that wheel. It is ~50MB compressed and ~155MB on disk
// (most of it a bundled Node runtime), which is exactly why Recur downloads it
// on demand instead of shipping it.

// pinnedSDKVersion is the cursor-sdk release Recur installs. The attribution
// patch in attribution.go matches minified anchors from this build — fetching
// "latest" would silently break those anchors on the next Cursor publish.
// Bump this only together with refreshed sdkAttributionAnchors.
const pinnedSDKVersion = "1.0.31"

// PinnedSDKVersion is the cursor-sdk release Recur installs.
func PinnedSDKVersion() string { return pinnedSDKVersion }

// pypiPinnedProject is the JSON metadata endpoint for the pinned wheel.
const pypiPinnedProject = "https://pypi.org/pypi/cursor-sdk/" + pinnedSDKVersion + "/json"

// pypiProjectJSON is the project-level PyPI metadata (includes latest version).
const pypiProjectJSON = "https://pypi.org/pypi/cursor-sdk/json"

// bridgeArchivePrefix is the path inside the wheel that holds the bridge.
const bridgeArchivePrefix = "cursor_sdk/_vendor/bridge/"

// installTimeout bounds the whole fetch+unpack.
const installTimeout = 15 * time.Minute

// InstallPhase names a step of the install, for progress reporting.
type InstallPhase string

const (
	PhaseResolving   InstallPhase = "resolving"
	PhaseDownloading InstallPhase = "downloading"
	PhaseVerifying   InstallPhase = "verifying"
	PhaseExtracting  InstallPhase = "extracting"
	PhaseDone        InstallPhase = "done"
)

// InstallProgress is one progress tick. Total is 0 when unknown.
type InstallProgress struct {
	Phase   InstallPhase `json:"phase"`
	Message string       `json:"message"`
	Current int64        `json:"current"`
	Total   int64        `json:"total"`
}

// Percent returns 0..100, or -1 when the total is unknown.
func (p InstallProgress) Percent() int {
	if p.Total <= 0 {
		return -1
	}
	return int(p.Current * 100 / p.Total)
}

// wheel is one downloadable file from the PyPI metadata.
type wheel struct {
	Filename string `json:"filename"`
	URL      string `json:"url"`
	Size     int64  `json:"size"`
	Digests  struct {
		SHA256 string `json:"sha256"`
	} `json:"digests"`
}

// Install downloads and unpacks the bridge into <dataDir>/cursor-bridge,
// returning the launcher path. progress may be nil.
//
// The install is staged in a sibling temp directory and moved into place only
// once it is complete, so an interrupted download can never leave a
// half-extracted bridge that Locate would happily hand to the engine.
func Install(ctx context.Context, dataDir string, progress func(InstallProgress)) (string, error) {
	if dataDir == "" {
		return "", fmt.Errorf("cursor bridge install: no data directory")
	}
	report := func(p InstallProgress) {
		if progress != nil {
			progress(p)
		}
	}

	ctx, cancel := context.WithTimeout(ctx, installTimeout)
	defer cancel()

	report(InstallProgress{Phase: PhaseResolving, Message: "Finding the Cursor SDK for this platform…"})
	w, version, err := resolveWheel(ctx)
	if err != nil {
		return "", err
	}

	dest := filepath.Join(dataDir, BridgeDirName)
	staging, err := os.MkdirTemp(dataDir, ".cursor-bridge-staging-*")
	if err != nil {
		return "", fmt.Errorf("cursor bridge install: staging dir: %w", err)
	}
	defer func() { _ = os.RemoveAll(staging) }()

	archive := filepath.Join(staging, "wheel.zip")
	report(InstallProgress{
		Phase:   PhaseDownloading,
		Message: fmt.Sprintf("Downloading Cursor SDK %s…", version),
		Total:   w.Size,
	})
	sum, err := download(ctx, w.URL, archive, func(n int64) {
		report(InstallProgress{
			Phase:   PhaseDownloading,
			Message: fmt.Sprintf("Downloading Cursor SDK %s…", version),
			Current: n,
			Total:   w.Size,
		})
	})
	if err != nil {
		return "", err
	}

	// PyPI publishes a sha256 per file; checking it means a truncated or
	// tampered download fails here rather than as a baffling Node crash later.
	report(InstallProgress{Phase: PhaseVerifying, Message: "Verifying download…"})
	if want := strings.ToLower(w.Digests.SHA256); want != "" && sum != want {
		return "", fmt.Errorf("cursor bridge install: checksum mismatch (got %s, want %s)", sum, want)
	}

	report(InstallProgress{Phase: PhaseExtracting, Message: "Extracting…"})
	unpacked := filepath.Join(staging, "bridge")
	if err := extractBridge(archive, unpacked, func(cur, total int64) {
		report(InstallProgress{Phase: PhaseExtracting, Message: "Extracting…", Current: cur, Total: total})
	}); err != nil {
		return "", err
	}

	launcher := filepath.Join(unpacked, launcherRelPath)
	if !isExecutableFile(launcher) {
		return "", fmt.Errorf("cursor bridge install: no launcher at %s after extraction", launcherRelPath)
	}

	// Wire attributionConfigProvider before the tree goes live. The wheel is
	// pinnedSDKVersion, so the minify anchors are expected to match — fail the
	// install if they don't rather than shipping a bridge that ignores Settings.
	report(InstallProgress{Phase: PhaseExtracting, Message: "Patching attribution controls…"})
	if err := EnsureAttributionPatch(unpacked); err != nil {
		return "", fmt.Errorf("cursor bridge install: attribution patch (sdk %s): %w", pinnedSDKVersion, err)
	}

	// Swap into place. Removing the old install first keeps the rename simple;
	// the window where neither exists is milliseconds and only matters if the
	// user reinstalls while a loop is running (the running bridge holds its own
	// open file handles and is unaffected).
	if err := os.RemoveAll(dest); err != nil {
		return "", fmt.Errorf("cursor bridge install: clear old install: %w", err)
	}
	if err := os.Rename(unpacked, dest); err != nil {
		return "", fmt.Errorf("cursor bridge install: move into place: %w", err)
	}

	report(InstallProgress{Phase: PhaseDone, Message: "Cursor SDK installed"})
	return filepath.Join(dest, launcherRelPath), nil
}

// latestSDKVersion returns the current cursor-sdk version published on PyPI
// (project "info.version"). Used by the pin-drift check — not by Install.
func latestSDKVersion(ctx context.Context) (string, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, pypiProjectJSON, nil)
	if err != nil {
		return "", err
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return "", fmt.Errorf("cursor sdk pin check: reach PyPI: %w", err)
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("cursor sdk pin check: PyPI returned %s", resp.Status)
	}
	var meta struct {
		Info struct {
			Version string `json:"version"`
		} `json:"info"`
	}
	if err := json.NewDecoder(io.LimitReader(resp.Body, 8<<20)).Decode(&meta); err != nil {
		return "", fmt.Errorf("cursor sdk pin check: decode PyPI metadata: %w", err)
	}
	if meta.Info.Version == "" {
		return "", fmt.Errorf("cursor sdk pin check: empty version in PyPI metadata")
	}
	return meta.Info.Version, nil
}

// resolveWheel picks the wheel matching this OS/arch from the pinned
// cursor-sdk release on PyPI (see pinnedSDKVersion).
//
// It MATCHES filenames rather than constructing a PEP 425 tag: the published
// tags vary in ways that are not worth predicting (macOS wheels are
// `macosx_11_0_*`, Linux ones carry a compound
// `manylinux2014_x86_64.manylinux_2_17_x86_64`), and a hardcoded tag would
// silently stop matching when Cursor rebuilds against a new baseline.
func resolveWheel(ctx context.Context) (wheel, string, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, pypiPinnedProject, nil)
	if err != nil {
		return wheel{}, "", err
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return wheel{}, "", fmt.Errorf("cursor bridge install: reach PyPI: %w", err)
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusOK {
		return wheel{}, "", fmt.Errorf("cursor bridge install: PyPI returned %s for cursor-sdk==%s", resp.Status, pinnedSDKVersion)
	}

	var meta struct {
		Info struct {
			Version string `json:"version"`
		} `json:"info"`
		URLs []wheel `json:"urls"`
	}
	if err := json.NewDecoder(io.LimitReader(resp.Body, 8<<20)).Decode(&meta); err != nil {
		return wheel{}, "", fmt.Errorf("cursor bridge install: decode PyPI metadata: %w", err)
	}
	if meta.Info.Version != pinnedSDKVersion {
		return wheel{}, "", fmt.Errorf("cursor bridge install: PyPI returned version %q, want pinned %s", meta.Info.Version, pinnedSDKVersion)
	}

	osTag, archTags, err := platformTags()
	if err != nil {
		return wheel{}, "", err
	}
	pinToken := "-" + strings.ToLower(pinnedSDKVersion) + "-"
	for _, w := range meta.URLs {
		name := strings.ToLower(w.Filename)
		if !strings.HasSuffix(name, ".whl") || !strings.Contains(name, osTag) {
			continue
		}
		// Refuse a wheel whose filename is not the pinned release (defense in
		// depth if PyPI ever serves the wrong urls list).
		if !strings.Contains(name, pinToken) {
			continue
		}
		for _, a := range archTags {
			if strings.Contains(name, a) {
				return w, pinnedSDKVersion, nil
			}
		}
	}
	return wheel{}, "", fmt.Errorf(
		"cursor bridge install: no cursor-sdk==%s wheel for %s/%s", pinnedSDKVersion, runtime.GOOS, runtime.GOARCH)
}

// platformTags returns the OS substring and acceptable arch substrings for this
// machine's wheel filename.
func platformTags() (string, []string, error) {
	var osTag string
	switch runtime.GOOS {
	case "darwin":
		osTag = "macosx"
	case "linux":
		osTag = "manylinux"
	case "windows":
		osTag = "win"
	default:
		return "", nil, fmt.Errorf("cursor bridge install: unsupported OS %q", runtime.GOOS)
	}

	switch runtime.GOARCH {
	case "arm64":
		return osTag, []string{"arm64", "aarch64"}, nil
	case "amd64":
		return osTag, []string{"x86_64", "amd64"}, nil
	default:
		return "", nil, fmt.Errorf("cursor bridge install: unsupported architecture %q", runtime.GOARCH)
	}
}

// download streams url to dst, reporting bytes written, and returns the hex
// sha256 of what it wrote.
func download(ctx context.Context, url, dst string, onProgress func(int64)) (string, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return "", err
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return "", fmt.Errorf("cursor bridge install: download: %w", err)
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("cursor bridge install: download returned %s", resp.Status)
	}

	f, err := os.Create(dst) //nolint:gosec // dst is an app-owned staging path
	if err != nil {
		return "", err
	}
	defer func() { _ = f.Close() }()

	h := sha256.New()
	pw := &progressWriter{onProgress: onProgress}
	if _, err := io.Copy(io.MultiWriter(f, h, pw), resp.Body); err != nil {
		return "", fmt.Errorf("cursor bridge install: download: %w", err)
	}
	if err := f.Sync(); err != nil {
		return "", err
	}
	return hex.EncodeToString(h.Sum(nil)), nil
}

// progressWriter counts bytes and throttles callbacks to once per ~1MB, so a
// 50MB download does not publish tens of thousands of UI events.
type progressWriter struct {
	n          int64
	lastReport int64
	onProgress func(int64)
}

const progressStride = 1 << 20

func (w *progressWriter) Write(p []byte) (int, error) {
	w.n += int64(len(p))
	if w.onProgress != nil && w.n-w.lastReport >= progressStride {
		w.lastReport = w.n
		w.onProgress(w.n)
	}
	return len(p), nil
}

// extractBridge unpacks just `cursor_sdk/_vendor/bridge/**` from the wheel into
// dst, preserving the executable bit — without it, `bin/node` and the launcher
// script come out non-executable and the bridge cannot start.
func extractBridge(archive, dst string, onProgress func(cur, total int64)) error {
	zr, err := zip.OpenReader(archive)
	if err != nil {
		return fmt.Errorf("cursor bridge install: open wheel: %w", err)
	}
	defer func() { _ = zr.Close() }()

	var members []*zip.File
	var total int64
	for _, f := range zr.File {
		if !strings.HasPrefix(f.Name, bridgeArchivePrefix) || strings.HasSuffix(f.Name, "/") {
			continue
		}
		members = append(members, f)
		total += int64(f.UncompressedSize64)
	}
	if len(members) == 0 {
		return fmt.Errorf("cursor bridge install: wheel contains no %s", bridgeArchivePrefix)
	}

	var done int64
	for _, f := range members {
		rel := strings.TrimPrefix(f.Name, bridgeArchivePrefix)
		// Zip-slip guard: a crafted archive must not be able to write outside dst.
		target := filepath.Join(dst, filepath.FromSlash(path.Clean(rel)))
		if !strings.HasPrefix(target, filepath.Clean(dst)+string(os.PathSeparator)) {
			return fmt.Errorf("cursor bridge install: refusing unsafe path %q in wheel", f.Name)
		}
		if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
			return err
		}
		if err := writeMember(f, target); err != nil {
			return err
		}
		done += int64(f.UncompressedSize64)
		if onProgress != nil {
			onProgress(done, total)
		}
	}
	return nil
}

// writeMember copies one zip entry to disk, carrying its mode across so the
// bundled binaries stay executable.
func writeMember(f *zip.File, target string) error {
	rc, err := f.Open()
	if err != nil {
		return err
	}
	defer func() { _ = rc.Close() }()

	mode := f.Mode().Perm()
	if mode == 0 {
		mode = 0o644
	}
	out, err := os.OpenFile(target, os.O_CREATE|os.O_TRUNC|os.O_WRONLY, mode) //nolint:gosec // app-owned staging path
	if err != nil {
		return err
	}
	defer func() { _ = out.Close() }()

	// Bounded to keep a hostile archive from filling the disk; the largest real
	// member is the ~120MB Node binary.
	if _, err := io.Copy(out, io.LimitReader(rc, 512<<20)); err != nil {
		return err
	}
	return out.Close()
}
