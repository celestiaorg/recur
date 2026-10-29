use async_trait::async_trait;
use recur_domain::Policy;

use crate::Result;

#[async_trait]
pub trait PolicyService: Send + Sync {
    async fn get_policy(&self) -> Result<Policy>;

    async fn set_policy(&self, p: &Policy) -> Result<Policy>;
}
