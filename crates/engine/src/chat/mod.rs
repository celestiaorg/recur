//! Chat and coordination: the workspace chat send pipeline and
//! its tabs, chat modes and the privilege-safe `switch_mode`, the looking-at
//! surface, forks, restart recovery, subagent child workspaces, the
//! `create_workspace` tool, the ask_user resume, and agents' background
//! processes and wakeups.

pub mod ask_resume;
pub mod children;
pub mod create_tool;
pub mod fork;
pub mod modes;
pub mod procs;
pub mod recover;
pub mod send;
pub mod surface;
pub mod switch_mode;
pub mod tabs;
pub mod wakeups;

#[cfg(test)]
mod tests;
