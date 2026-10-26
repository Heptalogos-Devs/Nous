use nous_domain::Result;
use nous_storage::WriteTransaction;
use std::{future::Future, pin::Pin};

pub enum AdmissionDecision {
    Ready,
    Refresh,
}
pub type AdmissionFuture<'a> = Pin<Box<dyn Future<Output = Result<AdmissionDecision>> + Send + 'a>>;

/// Domain preconditions run under the same writer transaction as the paid-call reservation.
pub trait CallAdmission<M>: Send + Sync {
    fn validate<'a>(
        &'a self,
        transaction: &'a mut WriteTransaction,
        metadata: &'a M,
    ) -> AdmissionFuture<'a>;
}
