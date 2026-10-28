//! PTY sessions and terminal emulation state.
//!
//! [`Manager`] owns the live shells (spawn, I/O, resize, flow control, replay, busy/idle
//! detection, interrupt and teardown) and publishes [`Event`]s. The emulator half lives in
//! [`screen`]: alacritty's terminal state the UI renders and feeds input through.

mod cwd;
pub mod input;
mod manager;
mod osc7;
mod plain;
mod process;
mod ring;
pub mod screen;

pub use manager::{
    Emitter, Error, Event, Info, Manager, OpenOptions, RESTORE_TRAILER, SessionSnapshot,
};
pub use plain::{DEFAULT_TAIL_LINES, MAX_TAIL_LINES, strip_vt, tail_plain};
pub use screen::{Output, Screen};
