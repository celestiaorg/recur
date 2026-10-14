package main

import (
	"context"
	"embed"
	"fmt"
	"io"
	"log"
	"log/slog"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"sync/atomic"
	"time"

	"github.com/wailsapp/wails/v3/pkg/application"
	"github.com/wailsapp/wails/v3/pkg/events"
	"github.com/wailsapp/wails/v3/pkg/services/dock"
	"github.com/wailsapp/wails/v3/pkg/services/notifications"
	"github.com/wailsapp/wails/v3/pkg/updater"

	"recur/internal/app"
	"recur/internal/bundles"
	"recur/internal/cliinstall"
	"recur/internal/code"
	"recur/internal/engine"
	"recur/internal/store"
)

//go:embed all:frontend/dist
var assets embed.FS

// Shipped Recur Bundles (repo-root bundles/). Embedded here because go:embed
// cannot reach outside the declaring package — not duplicated under internal/.
//
//go:embed all:bundles
var shippedBundles embed.FS

// Shipped Recur-owned Agent Skills (repo-root recur-skills/). Seeded into
// ~/.recur/recur-skills on boot with the same stamp/sync pattern as bundles.
//
//go:embed all:recur-skills
var shippedSkills embed.FS

// Window/taskbar icon on Linux; macOS takes it from the .app bundle.
//
//go:embed build/appicon.png
var appIcon []byte

func main() {
	// Updater.Restart re-execs this binary as the swap helper while Recur
	// still holds the instance lock; application.New would divert it too
	// late, after claimOrHandoff handed it off to the running app.
	updater.HandleHelperMode()

	// Clicking an OS notification (or re-opening Recur.app) starts a new
	// process. Own the data dir (flock + socket) before opening the store —
	// a second Recover() would park in-flight loops. Claim is atomic; a
	// post-check listener is not (two launches can both miss an unbound
	// socket).
	inst, handed, err := claimOrHandoff(coldOpenPaths)
	if handed {
		return
	}
	if err != nil {
		log.Fatal(err)
	}
	defer inst.Close()
	gate := newIPCGate()
	hub := &openHub{}
	stopIPC := startOpenIPC(inst, gate, hub)

	// Tee stdlib log and runtime crash traces to recur.log before anything
	// else so store-open failures and unguarded panics survive a Dock-launched
	// .app (stderr is discarded). Wails gets its own slog logger on the same
	// writer — do not slog.SetDefault, which reformats log.Printf.
	var appLog io.Writer
	if root, rerr := store.DataDir(); rerr == nil {
		var stop func()
		appLog, stop = store.TeeAppLog(root)
		defer stop()
	}

	// Dock-launched Recur.app inherits launchd PATH. Probe the login shell
	// (and merge common bin dirs) before any LookPath (claude, gh, ...).
	cliinstall.EnsurePATH()

	// Engine owns the store + event bus + governor + agent runner + worktree
	// manager (PRD §6.1). The store is SQLite-backed under the managed root; a
	// fresh app starts empty. The production engine uses the real ClaudeRunner
	// so a live session works on the user's login (DECISIONS D8).
	st, err := store.New()
	if err != nil {
		log.Fatalf("open store: %v", err)
	}
	defer st.Close()

	// Reload native-drop allowlist so /external-file and /chat-image still
	// serve files the user dropped last run (transcript chips, restored tabs).
	if root, rerr := store.DataDir(); rerr == nil {
		code.LoadExternalAllowFile(filepath.Join(root, "external-allow.json"))
	}

	// Seed shipped Recur Bundles into <DataDir>/Bundles on first launch (and
	// whenever a default file is missing). User bundles live in ~/.recur/bundles.
	if bdir, berr := store.BundlesDir(); berr == nil {
		if serr := bundles.SeedDefaults(bdir, shippedBundles, "bundles"); serr != nil {
			log.Printf("seed default bundles: %v", serr)
		}
	}
	// Seed shipped Recur skills into ~/.recur/recur-skills (sync when embedded
	// content changes). Project/user skills still win on name collision.
	if sdir, serr := store.RecurSkillsDir(); serr == nil {
		if err := bundles.SeedDefaults(sdir, shippedSkills, "recur-skills"); err != nil {
			log.Printf("seed recur skills: %v", err)
		}
	}

	eng := engine.New(st)

	// Reconcile loops AND chat tabs that were persisted mid-run when the app last
	// exited (crash / force-quit / quit): they have no goroutine, so Recover parks
	// loops as paused/interrupted and idles zombie chat agents. AutoResume then
	// continues both (PRD §6.5, D23). Must run Recover before the UI so a
	// restart does not paint chats busy with nothing running.
	eng.Recover()

	svc := app.NewServices(eng)
	app.StartProviderUsage(svc.Settings)
	defer app.StopProviderUsage(svc.Settings)
	attachOpenIPC(gate, svc, hub)
	applyColdOpens(svc, hub, coldOpenPaths)

	// notif backs OS-native "needs you" / "PR ready" notifications (Phase 3.6). The
	// engine stays Wails-free — it only publishes a generic "notify" bus event; the
	// subscriber below turns that into a native notification.
	notif := notifications.New()
	// Official Wails Dock service (https://v3.wails.io/features/platform/dock/).
	// Badge updates call dockService.SetBadge / RemoveBadge from Go (docs +
	// examples/badge "set via Go" path). Never call before wapp.Run — AppKit
	// dispatch_sync to the main queue SIGTRAPs / deadlocks if NSApp is not live.
	dockService := dock.New()

	services := []application.Service{
		application.NewService(svc.Projects),
		application.NewService(svc.Workspaces),
		application.NewService(svc.Maestro),
		application.NewService(svc.Escalation),
		application.NewService(svc.Permissions),
		application.NewService(svc.Questions),
		application.NewService(svc.Artifacts),
		application.NewService(svc.Agents),
		application.NewService(svc.PRs),
		application.NewService(svc.Policy),
		application.NewService(svc.Inbox),
		application.NewService(svc.Settings),
		application.NewService(svc.Code),
		application.NewService(svc.Observatory),
		application.NewService(svc.Terminal),
		application.NewService(svc.Bundle),
		application.NewService(svc.EditorSession),
		application.NewService(svc.Shell),
		application.NewService(svc.LSP),
		application.NewService(svc.Briefing),
		application.NewService(dockService),
	}
	// Registering the notifications service runs its ServiceStartup, which on
	// darwin hard-fails ("notifications require a valid bundle identifier") for any
	// unbundled run (`wails3 task dev`, `go run .`) — and a failed service startup
	// aborts wapp.Run() entirely, taking the whole app down with it. So it's only
	// registered from a real .app bundle; unbundled runs skip native notifications
	// altogether and sendOSNotification's osascript fallback carries the load
	// (both call sites are separately gated on runningInAppBundle() too).
	if runningInAppBundle() {
		services = append(services, application.NewService(notif))
	}

	opts := application.Options{
		Name:        "Recur",
		Description: "Run autonomous AI coding loops locally.",
		Services:    services,
		Assets: application.AssetOptions{
			Handler: application.AssetFileServerFS(assets),
			// Serve per-workspace managed files (attachments, screenshots, artifacts)
			// at /workspaces/<id>/… Path-traversal-safe under DataDir/workspaces/.
			//
			// /workspace-file streams worktree bytes the same way so observatory
			// opens do not JSON-parse megabytes on the Wails bridge.
			// /chat-image streams an absolute-path png/jpeg/gif/webp so Finder
			// drops onto chat can attach like paste (native FileList is empty).
			// /external-file streams an absolute-path editor file (text/image)
			// so clicking an outside-worktree @file chip can open it.
			// /gh-image streams a GitHub attachment with the user's gh token so
			// PR description/comment images render (user-attachments 404 without auth).
			Middleware: func(next http.Handler) http.Handler {
				return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
					if strings.HasPrefix(r.URL.Path, "/workspaces/") {
						serveWorkspaceData(w, r)
						return
					}
					if r.URL.Path == "/workspace-file" {
						app.ServeWorkspaceFile(svc.Code, w, r)
						return
					}
					if r.URL.Path == "/chat-image" {
						app.ServeChatImage(w, r)
						return
					}
					if r.URL.Path == "/external-file" {
						app.ServeExternalFile(w, r)
						return
					}
					if r.URL.Path == "/gh-image" {
						app.ServeGitHubImage(w, r)
						return
					}
					next.ServeHTTP(w, r)
				})
			},
		},
		Mac: application.MacOptions{
			ApplicationShouldTerminateAfterLastWindowClosed: true,
		},
	}
	if runtime.GOOS != "darwin" {
		opts.Icon = appIcon
	}
	if appLog != nil {
		opts.Logger = slog.New(slog.NewTextHandler(appLog, nil))
	}
	wapp := application.New(opts)

	// Fan the event bus out to the Wails runtime (PRD §6.3: Bus → Wails emitter).
	// The durable-events persistence sink is written by the engine at the point of
	// emit (engine.persistEvent → store append-only events table; PRD §6.5, D13),
	// so the Bus here only needs the Wails emitter.
	//
	// Dock badge (D-attention): Go dockService.SetBadge with AttentionCounts().Dock
	// (unseen chats + non-chat inbox unread; chat rows are inbox items).
	// (https://v3.wails.io/features/platform/dock/). Modern macOS needs Notifications
	// → Badge authorization or setBadgeLabel is ignored. Call only after Run is
	// pumping; SetBadge before Run SIGTRAPs (AppKit main-queue sync).
	var dockLive atomic.Bool
	syncDockBadge := func() {
		if !dockLive.Load() {
			return
		}
		n := eng.AttentionCounts().Dock
		if n <= 0 {
			_ = dockService.RemoveBadge()
			return
		}
		_ = dockService.SetBadge(strconv.Itoa(n))
	}

	eng.Bus.Subscribe(func(ev engine.Event) {
		wapp.Event.Emit(ev.Name, ev.Data)
		if ev.Name == "notify" {
			if data, ok := ev.Data.(map[string]any); ok {
				title, _ := data["title"].(string)
				body, _ := data["body"].(string)
				go sendOSNotification(notif, title, body) // never block the bus
			}
		}
		if ev.Name == "inbox.updated" || ev.Name == "attention.updated" {
			// Off the bus goroutine: SetBadge uses dispatch_sync(main).
			go syncDockBadge()
		}
	})

	// After Run is pumping: request notification+badge auth (bundled .app only —
	// UNUserNotificationCenter SIGABRTs unbundled binaries), then apply.
	go func() {
		time.Sleep(500 * time.Millisecond)
		dockLive.Store(true)
		if runningInAppBundle() {
			if ok, _ := notif.CheckNotificationAuthorization(); !ok {
				_, _ = notif.RequestNotificationAuthorization()
			}
		}
		syncDockBadge()
	}()

	// Auto-resume loops Recover parked as interrupted, and coding/Maestro chats
	// it queued the same way, so closing and reopening the app CONTINUES the
	// work instead of leaving loops waiting on Resume and chats stuck busy with
	// no turn (D55, PRD §6.5). Deliberately placed after the Bus → Wails wiring
	// above so the resumed work's events reach the UI, and run on its own
	// goroutine because re-dispatch does synchronous git work per loop (worktree
	// recreate, up to 60s each) which must not delay the window appearing.
	// Check git once the bus is wired so a too-old install raises notify +
	// resource.gitWarning before the window loads. Auto-resume stays async.
	eng.CheckGit()
	eng.CheckCursorBridge()
	if err := eng.StartScheduledPrompts(); err != nil {
		log.Printf("scheduled prompts: %v", err)
	}
	go eng.AutoResume()

	// Production .app only: symlink the bundled CLI onto PATH (D-cli-install).
	// Dev / `go run` / missing Contents/MacOS/recur-cli are no-ops.
	go installBundledCLI()

	// Recur window: dark, frameless-inset title bar so the ported chrome sits beside
	// the native macOS traffic-light buttons (see DECISIONS D-window). --bg = #0A0C10.
	// Size/position are restored from ~/Library/Application Support/Recur/window.json.
	win := wapp.Window.NewWithOptions(windowOptionsFromState(loadWindowState()))
	wireOpenHub(wapp, win, hub)
	// Notification clicks are delivered here when macOS activates this
	// process instead of spawning another. Focus the existing window.
	notif.OnNotificationResponse(func(notifications.NotificationResult) {
		go hub.bringForward()
	})
	persistWindowGeometry(win)
	wireTitlebarDoubleClick(wapp, win)
	win.OnWindowEvent(events.Common.WindowFilesDropped, func(event *application.WindowEvent) {
		files := event.Context().DroppedFiles()
		if len(files) == 0 {
			return
		}
		for _, f := range files {
			code.AllowExternalPath(f)
		}
		payload := map[string]any{"files": files}
		if d := event.Context().DropTargetDetails(); d != nil {
			payload["attributes"] = d.Attributes
			payload["id"] = d.ElementID
			payload["classList"] = d.ClassList
		}
		wapp.Event.Emit("files.dropped", payload)
	})
	// RegisterHook runs synchronously before Wails' async WindowClosing
	// listeners (including the one that destroys the window). OnWindowEvent
	// would race destroy and let an in-flight IPC Show() deadlock stopIPC.
	// Stop the gate before waitFocus so a reopen during drain gets
	// ErrShuttingDown instead of OK from a no-op bringForward.
	win.RegisterHook(events.Common.WindowClosing, func(*application.WindowEvent) {
		revokeOpenUI(hub, gate)
		hub.waitFocus()
		svc.Shell.Flush()
	})
	// Recur > Quit runs Wails cleanup, which closes windows after
	// OnShutdown and may never deliver WindowClosing (map already nil).
	// Drop UI and stop IPC here, before native teardown; do not waitFocus
	// (this runs on the main thread via InvokeSync). dropUI unblocks any
	// bringForward still waiting for the Show callback.
	wapp.OnShutdown(func() {
		revokeOpenUI(hub, gate)
	})
	// D-attention: a finish on the selected tab still counts as unseen when the
	// window is not key (user is in another app). Restoring focus marks the
	// selected tab seen when no UI overlay is covering it.
	win.OnWindowEvent(events.Common.WindowLostFocus, func(*application.WindowEvent) {
		eng.SetAppFocused(false)
	})
	win.OnWindowEvent(events.Common.WindowFocus, func(*application.WindowEvent) {
		eng.SetAppFocused(true)
	})

	// Real loops are started on demand via WorkspacesService.StartWorkspace (no fake emitter).
	// On quit, cancel every running loop AND join their goroutines (bounded deadline)
	// so agents' process groups are reaped before the app exits (D24). This must be
	// a Wails shutdown hook, not a defer: on macOS Cmd+Q runs the hooks from
	// applicationShouldTerminate and AppKit then exit()s — [NSApp run] never
	// returns, so anything deferred past wapp.Run() is skipped and the claude/
	// codex process groups (Setpgid) outlive the app.
	wapp.OnShutdown(func() {
		// Stop accepting (and join in-flight OpenPath) before Shutdown/store
		// close. The flock stays held until inst.Close.
		stopIPC()
		eng.Shutdown()
	})

	installAppMenu(wapp, installUpdater(wapp))

	if err := wapp.Run(); err != nil {
		log.Fatal(err)
	}
}

// serveWorkspaceData serves a file from <DataDir>/workspaces/ at
// /workspaces/<workspaceID>/{attachments,screenshots,artifacts}/…
// It refuses anything that escapes the workspaces root (traversal-safe).
func serveWorkspaceData(w http.ResponseWriter, r *http.Request) {
	root, err := store.DataDir()
	if err != nil {
		http.NotFound(w, r)
		return
	}
	base := filepath.Join(root, "workspaces")
	rel := strings.TrimPrefix(r.URL.Path, "/workspaces/")
	full := filepath.Join(base, filepath.Clean("/"+rel)) // Clean("/x") strips any ../
	if full != base && !strings.HasPrefix(full, base+string(filepath.Separator)) {
		http.NotFound(w, r)
		return
	}
	http.ServeFile(w, r, full)
}

// runningInAppBundle reports whether this binary is running from inside a proper
// macOS .app bundle (…/Recur.app/Contents/MacOS/recur). The native notifications
// path requires this — see the call site in main() for why it's not just a
// nice-to-have gate but a crash-prevention one.
func runningInAppBundle() bool {
	exe, err := os.Executable()
	if err != nil {
		return false
	}
	return strings.Contains(exe, ".app/Contents/MacOS/")
}

func installBundledCLI() {
	exe, err := os.Executable()
	if err != nil {
		return
	}
	if err := cliinstall.InstallFromApp(exe); err != nil {
		log.Printf("cli install: %v", err)
	}
}

// dockBadgeCount is the documented dock arithmetic: unseen chats + other inbox
// unread. Chat rows live in the inbox, so pass non-chat unread here (or 0 when
// the caller already counted chats via unseen).
func dockBadgeCount(inboxUnread, unseenChats int) int {
	if inboxUnread < 0 {
		inboxUnread = 0
	}
	if unseenChats < 0 {
		unseenChats = 0
	}
	return inboxUnread + unseenChats
}

// sendOSNotification delivers a native OS notification for the engine's "notify"
// bus event (Phase 3.6: escalations + PR-ready). The native UNUserNotificationCenter
// path requires a signed, bundled .app (main() only calls this expecting the native
// leg to work when runningInAppBundle(); see its comment for the crash this guards
// against), so unbundled runs (dev, `go run .`) skip straight to
// `osascript -e 'display notification'` (darwin) or `notify-send` (Linux, where
// the Wails service is skipped because it aborts startup without a D-Bus
// session). Best-effort and bounded so a hung helper can never wedge anything;
// failures are logged only.
func sendOSNotification(notif *notifications.NotificationService, title, body string) {
	if title == "" {
		return
	}
	if runningInAppBundle() {
		if err := notif.SendNotification(notifications.NotificationOptions{
			ID:    fmt.Sprintf("recur-%d", time.Now().UnixNano()),
			Title: title,
			Body:  body,
		}); err == nil {
			return
		}
	}
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "notify-send", "--app-name=Recur", title, body)
	if runtime.GOOS == "darwin" {
		script := fmt.Sprintf("display notification %q with title %q", body, title)
		cmd = exec.CommandContext(ctx, "osascript", "-e", script)
	}
	if out, oerr := cmd.CombinedOutput(); oerr != nil {
		log.Printf("notification failed: %v: %s", oerr, out)
	}
}
