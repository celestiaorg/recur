use async_trait::async_trait;
use recur_domain::InboxItem;

use crate::Result;

/// Rust port of Go `app.InboxService` (`frontend/bindings/recur/internal/app/inboxservice.ts`).
#[async_trait]
pub trait InboxService: Send + Sync {
    async fn get_inbox(&self) -> Result<Vec<InboxItem>>;

    async fn mark_all_inbox_read(&self) -> Result<i64>;

    async fn mark_inbox_read(&self, id: &str) -> Result<i64>;
}
