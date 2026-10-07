// Package engine is the concurrency spine (PRD §6): one goroutine per loop, the
// event Bus (fan-in from loops → fan-out to UI), the resource Governor, and the
// agent runners. Real per-loop goroutines run in M2.
package engine

import "sync"

// Event is anything a loop emits. Name mirrors the PRD §15.2 event names exactly
// (loop.updated, loop.event, agent.step, …). Data is the JSON payload.
type Event struct {
	Name string
	Data any
}

// Sink consumes one event. In M2 the Bus has a single sink: the Wails emitter
// (fan-out to the UI). Durable persistence is NOT a Bus sink — the engine persists
// state-changing events INLINE at the point of emit (engine.persistEvent → the
// append-only events table; PRD §6.5, DECISIONS D13/D19).
type Sink func(Event)

// Bus fans events in from all loops and out to every registered sink. State-changing
// events must never be dropped (PRD §6.3); M0 publishes synchronously.
type Bus struct {
	mu    sync.RWMutex
	sinks []Sink
}

// NewBus returns an empty Bus.
func NewBus() *Bus { return &Bus{} }

// Subscribe registers a sink. Sinks are invoked synchronously in registration order.
func (b *Bus) Subscribe(s Sink) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.sinks = append(b.sinks, s)
}

// Publish fans an event out to every sink.
func (b *Bus) Publish(ev Event) {
	b.mu.RLock()
	sinks := append([]Sink(nil), b.sinks...)
	b.mu.RUnlock()
	for _, s := range sinks {
		s(ev)
	}
}
