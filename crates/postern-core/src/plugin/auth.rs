//! Authentication plugin trait (detailed design 4.1; module design 5.2,
//! step [1]). Core only DECLARES the shape; the identity/credential plane
//! implements the kinds (`local_process` / `api_key` / `token`; mTLS / SSO
//! once scaled up).

use crate::domain::{CredentialView, PresentedCredential, PrincipalId, Timestamp};
use crate::error::AuthError;
use crate::request::ConnOrigin;

/// Authenticator family (step [1]).
///
/// The presented credential is consumed by reference only; `now` is passed
/// explicitly (aligned with the evaluator) so that `expires_at` /
/// `revoked_at` / trust-domain validity are re-checked against the
/// evaluation wall clock - expiry takes effect immediately, never dependent
/// on a background sweeper's timing (detailed design 6.2).
pub trait Authenticator: Send + Sync {
    /// Authenticator-registry selection key (`local_process` / `api_key` /
    /// `token`); step [1] picks the implementation by the presented
    /// credential's kind.
    fn kind(&self) -> &'static str;

    /// Resolves the presented credential to a principal. Invalid / expired /
    /// revoked / trust-domain mismatch / undeterminable origin all return
    /// `Err`, which denies (axiom two).
    fn authenticate(
        &self,
        presented: &PresentedCredential,
        origin: &ConnOrigin,
        creds: &CredentialView,
        now: Timestamp,
    ) -> Result<PrincipalId, AuthError>;
}
