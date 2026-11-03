//! One interactive turn's reply collector: the assistant text for the call's
//! return value, the provider session id on init, and the Plan-mode createPlan
//! harvest. Live streaming to the UI is the broadcast's job, not this.

use std::sync::Arc;

use parking_lot::Mutex;
use recur_agent::AgentEvent;

use super::Emit;
use crate::Engine;

type OnInit = Box<dyn Fn(&str) + Send + Sync>;

#[derive(Default)]
struct Reply {
    text: String,
    got_delta: bool,
}

pub struct ChatStream {
    eng: Arc<Engine>,
    workspace_id: String,
    agent_code: String,
    /// Adopt Cursor createPlan bodies into the workspace plan (Plan mode).
    harvest_create_plan: bool,
    on_init: Option<OnInit>,
    reply: Mutex<Reply>,
}

impl ChatStream {
    pub fn new(eng: Arc<Engine>, workspace_id: &str, agent_code: &str) -> Arc<Self> {
        Self::with(eng, workspace_id, agent_code, false, None)
    }

    pub fn with(
        eng: Arc<Engine>,
        workspace_id: &str,
        agent_code: &str,
        harvest_create_plan: bool,
        on_init: Option<OnInit>,
    ) -> Arc<Self> {
        Arc::new(Self {
            eng,
            workspace_id: workspace_id.to_owned(),
            agent_code: agent_code.to_owned(),
            harvest_create_plan,
            on_init,
            reply: Mutex::default(),
        })
    }

    /// The turn's accumulated assistant text.
    pub fn text(&self) -> String {
        self.reply.lock().text.trim().to_owned()
    }

    /// This stream as a turn's caller callback.
    pub fn emit(self: &Arc<Self>) -> Emit {
        let me = Arc::clone(self);
        Arc::new(move |ev: &AgentEvent| me.on_event(ev))
    }

    fn on_event(&self, ev: &AgentEvent) {
        if ev.nested() {
            return;
        }
        match ev {
            AgentEvent::TextDelta { text, .. } => {
                let mut r = self.reply.lock();
                r.got_delta = true;
                r.text.push_str(text);
            }
            AgentEvent::Text { text, .. } => {
                let mut r = self.reply.lock();
                if !r.got_delta {
                    r.text.push_str(text);
                }
            }
            AgentEvent::Init { session_id, .. } => {
                if let Some(f) = &self.on_init {
                    f(session_id);
                }
            }
            AgentEvent::PlanProposal { body } if self.harvest_create_plan => {
                self.eng
                    .adopt_prose_plan(&self.workspace_id, &self.agent_code, body);
            }
            _ => {}
        }
    }
}
