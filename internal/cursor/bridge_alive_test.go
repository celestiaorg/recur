package cursor

import (
	"errors"
	"fmt"
	"net"
	"syscall"
	"testing"
)

func TestBridgeAlive(t *testing.T) {
	if (&Bridge{}).Alive() {
		t.Fatal("zero Bridge must not be alive")
	}
	if (*Bridge)(nil).Alive() {
		t.Fatal("nil Bridge must not be alive")
	}

	b := &Bridge{reaped: make(chan struct{})}
	if !b.Alive() {
		t.Fatal("open reaped channel means the process is still running")
	}

	close(b.reaped)
	if b.Alive() {
		t.Fatal("closed reaped channel means the process has exited")
	}

	b2 := &Bridge{reaped: make(chan struct{}), closed: true}
	if b2.Alive() {
		t.Fatal("Close()-ed Bridge must not be alive even before Wait returns")
	}
}

func TestBridgeUnreachable(t *testing.T) {
	if BridgeUnreachable(nil) {
		t.Fatal("nil is not unreachable")
	}
	if BridgeUnreachable(errors.New("cursor bridge: [invalid_argument] bad model")) {
		t.Fatal("Connect application errors are not bridge-dead")
	}
	if BridgeUnreachable(&Error{Code: "unauthenticated", Message: "no key"}) {
		t.Fatal("auth failures are not bridge-dead")
	}

	refused := fmt.Errorf(
		`cursor create agent: cursor bridge: sdk.v1.SdkAgentService/CreateAgent: Post "http://127.0.0.1:61765/sdk.v1.SdkAgentService/CreateAgent": %w`,
		&net.OpError{Op: "dial", Net: "tcp", Err: syscall.ECONNREFUSED},
	)
	if !BridgeUnreachable(refused) {
		t.Fatalf("want unreachable for connection refused, got false for %v", refused)
	}

	prose := errors.New(`cursor create agent: Post "http://127.0.0.1:61765/...": dial tcp 127.0.0.1:61765: connect: connection refused`)
	if !BridgeUnreachable(prose) {
		t.Fatal("want unreachable for connection-refused prose")
	}
}
