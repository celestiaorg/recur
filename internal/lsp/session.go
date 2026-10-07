package lsp

import (
	"bufio"
	"bytes"
	"fmt"
	"io"
	"log/slog"
	"os"
	"os/exec"
	"sync"
	"syscall"
)

// Emitter publishes an event to the frontend. Injected so this package stays
// free of Wails and the engine.
type Emitter func(name string, data any)

type lspEvent struct {
	name string
	data any
}

// Session is one long-lived language server process for a workspace.
type Session struct {
	ID            string
	WorkspaceID   string
	ServerKey     string
	RootURI       string
	RootPath      string
	VuePluginPath string

	cmd    *exec.Cmd
	stdin  io.WriteCloser
	stdout io.ReadCloser
	emit   Emitter

	mu           sync.Mutex
	writeMu      sync.Mutex
	eventMu      sync.Mutex
	closed       bool
	exitSent     bool
	eventsClosed bool

	events chan lspEvent
	quit   chan struct{}
}

func startSession(id, workspaceID, rootPath, rootURI string, spec ServerSpec, emit Emitter) (*Session, error) {
	cmd := exec.Command(spec.Command, spec.Args...)
	cmd.Dir = rootPath
	cmd.Env = os.Environ()
	if len(spec.Env) > 0 {
		cmd.Env = append(cmd.Env, spec.Env...)
	}
	// Own process group so Close can signal the whole tree.
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}

	stdin, err := cmd.StdinPipe()
	if err != nil {
		return nil, err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		_ = stdin.Close()
		return nil, err
	}
	cmd.Stderr = &logWriter{key: spec.Key}

	if err := cmd.Start(); err != nil {
		_ = stdin.Close()
		_ = stdout.Close()
		return nil, err
	}

	s := &Session{
		ID:          id,
		WorkspaceID: workspaceID,
		ServerKey:   spec.Key,
		RootURI:     rootURI,
		RootPath:    rootPath,
		cmd:         cmd,
		stdin:       stdin,
		stdout:      stdout,
		emit:        emit,
		events:      make(chan lspEvent, 64),
		quit:        make(chan struct{}),
	}
	go s.emitLoop()
	go s.readLoop()
	go s.waitLoop()
	return s, nil
}

func (s *Session) emitLoop() {
	for ev := range s.events {
		if s.emit != nil {
			s.emit(ev.name, ev.data)
		}
	}
}

func (s *Session) sendEvent(name string, data any) {
	s.eventMu.Lock()
	defer s.eventMu.Unlock()
	if s.eventsClosed {
		return
	}
	s.events <- lspEvent{name: name, data: data}
}

func (s *Session) stopEvents() {
	s.eventMu.Lock()
	defer s.eventMu.Unlock()
	if s.eventsClosed {
		return
	}
	s.eventsClosed = true
	close(s.quit)
	close(s.events)
}

func (s *Session) readLoop() {
	defer func() {
		s.emitExit()
		s.stopEvents()
	}()
	r := bufio.NewReaderSize(s.stdout, 256<<10)
	for {
		body, err := ReadFrame(r)
		if err != nil {
			if !s.isClosed() && err != io.EOF {
				slog.Debug("lsp read ended", "server", s.ServerKey, "err", err)
			}
			return
		}
		s.sendEvent("lsp.message", map[string]any{
			"id":      s.ID,
			"message": string(body),
		})
	}
}

func (s *Session) waitLoop() {
	err := s.cmd.Wait()
	if err != nil && !s.isClosed() {
		slog.Debug("lsp server exited", "server", s.ServerKey, "err", err)
	}
}

func (s *Session) emitExit() {
	s.mu.Lock()
	if s.exitSent {
		s.mu.Unlock()
		return
	}
	s.exitSent = true
	s.closed = true
	id := s.ID
	s.mu.Unlock()
	s.sendEvent("lsp.exit", map[string]any{"id": id})
}

func (s *Session) isClosed() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.closed
}

// Write sends one JSON-RPC message (no headers) to the server.
func (s *Session) Write(message string) error {
	s.mu.Lock()
	if s.closed {
		s.mu.Unlock()
		return fmt.Errorf("lsp session closed")
	}
	stdin := s.stdin
	s.mu.Unlock()

	s.writeMu.Lock()
	defer s.writeMu.Unlock()
	return WriteFrame(stdin, []byte(message))
}

// Close terminates the server process group.
func (s *Session) Close() {
	s.mu.Lock()
	if s.closed {
		s.mu.Unlock()
		return
	}
	s.closed = true
	stdin := s.stdin
	var procPid int
	if s.cmd != nil && s.cmd.Process != nil {
		procPid = s.cmd.Process.Pid
	}
	s.mu.Unlock()

	if stdin != nil {
		_ = stdin.Close()
	}
	if procPid != 0 {
		_ = syscall.Kill(-procPid, syscall.SIGTERM)
	}
}

type logWriter struct {
	key string
}

func (w *logWriter) Write(p []byte) (int, error) {
	msg := string(bytes.TrimSpace(p))
	if msg != "" {
		slog.Debug("lsp stderr", "server", w.key, "msg", msg)
	}
	return len(p), nil
}
