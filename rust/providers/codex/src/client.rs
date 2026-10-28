//! A typed façade over [`Conn`] for the methods Recur uses (Go
//! `codex/client.go`).

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::rpc::{Conn, Ctx, RpcError};
use crate::types::{
    AccountReadResult, LoginStartResult, Model, Thread, ThreadStartParams, Turn,
    TurnInterruptParams, TurnStartParams, TurnSteerParams,
};

#[derive(Clone)]
pub struct Client {
    conn: Conn,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ThreadOut {
    thread: Thread,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct TurnOut {
    turn: Turn,
}

impl Client {
    pub fn new(conn: Conn) -> Self {
        Self { conn }
    }

    pub fn conn(&self) -> &Conn {
        &self.conn
    }

    pub async fn account_read(&self, ctx: &Ctx) -> Result<AccountReadResult, RpcError> {
        ctx.run(
            self.conn
                .call("account/read", &json!({"refreshToken": false})),
        )
        .await
    }

    pub async fn account_login_start(&self, ctx: &Ctx) -> Result<LoginStartResult, RpcError> {
        ctx.run(
            self.conn
                .call("account/login/start", &json!({"type": "chatgpt"})),
        )
        .await
    }

    pub async fn account_login_cancel(&self, ctx: &Ctx, login_id: &str) -> Result<(), RpcError> {
        ctx.run(
            self.conn
                .call::<_, Value>("account/login/cancel", &json!({"loginId": login_id})),
        )
        .await
        .map(drop)
    }

    /// Visible models; `limit <= 0` means 50.
    pub async fn model_list(&self, ctx: &Ctx, limit: i64) -> Result<Vec<Model>, RpcError> {
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Out {
            data: Vec<Model>,
        }
        let limit = if limit <= 0 { 50 } else { limit };
        let out: Out = ctx
            .run(self.conn.call(
                "model/list",
                &json!({"includeHidden": false, "limit": limit}),
            ))
            .await?;
        Ok(out.data)
    }

    pub async fn thread_start(
        &self,
        ctx: &Ctx,
        mut p: ThreadStartParams,
    ) -> Result<Thread, RpcError> {
        if p.service_name.is_empty() {
            p.service_name = "recur".into();
        }
        let out: ThreadOut = ctx.run(self.conn.call("thread/start", &p)).await?;
        Ok(out.thread)
    }

    /// Resumes a thread with the start params' non-empty overrides.
    pub async fn thread_resume(
        &self,
        ctx: &Ctx,
        thread_id: &str,
        p: &ThreadStartParams,
    ) -> Result<Thread, RpcError> {
        let mut params = BTreeMap::from([("threadId", thread_id)]);
        for (k, v) in [
            ("model", &p.model),
            ("cwd", &p.cwd),
            ("approvalPolicy", &p.approval_policy),
            ("sandbox", &p.sandbox),
            ("developerInstructions", &p.developer_instructions),
        ] {
            if !v.is_empty() {
                params.insert(k, v);
            }
        }
        let out: ThreadOut = ctx.run(self.conn.call("thread/resume", &params)).await?;
        Ok(out.thread)
    }

    pub async fn turn_start(&self, ctx: &Ctx, p: &TurnStartParams) -> Result<Turn, RpcError> {
        let out: TurnOut = ctx.run(self.conn.call("turn/start", p)).await?;
        Ok(out.turn)
    }

    /// The steered turn id; empty when a build omits it.
    pub async fn turn_steer(&self, ctx: &Ctx, p: &TurnSteerParams) -> Result<String, RpcError> {
        #[derive(Default, Deserialize)]
        #[serde(default, rename_all = "camelCase")]
        struct Out {
            turn_id: String,
        }
        let out: Out = ctx.run(self.conn.call("turn/steer", p)).await?;
        Ok(out.turn_id)
    }

    pub async fn turn_interrupt(
        &self,
        ctx: &Ctx,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<(), RpcError> {
        ctx.run(self.conn.call::<_, Value>(
            "turn/interrupt",
            &TurnInterruptParams {
                thread_id: thread_id.into(),
                turn_id: turn_id.into(),
            },
        ))
        .await
        .map(drop)
    }
}
