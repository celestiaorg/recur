use async_trait::async_trait;

use crate::Result;

pub use recur_domain::insights::*;

/// InsightsService reports how agents spent their time over a range, for the Insights view.
#[async_trait]
pub trait InsightsService: Send + Sync {
    /// The report for `[start_ms, end_ms)` (unix ms, local day boundaries chosen by the
    /// caller). Comparisons use as long a stretch from `prev_start_ms`, the previous period's
    /// start, so a period in progress compares to date; the heatmap covers the 53 weeks ending
    /// at `end_ms`.
    async fn insights(
        &self,
        prev_start_ms: i64,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<InsightsReport>;
}
