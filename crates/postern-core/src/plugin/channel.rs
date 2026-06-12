//! Transport / data-plane plugin traits and their IO carriers (detailed
//! design 4.1; module design 5.2). Core only DECLARES these shapes; the
//! transports and adapters crates implement them.
//!
//! Secret-family types (`ResolvedTarget` / `ResourceCredential`) appear
//! ONLY by value in `Transport::open` / `CredentialProvider::credential_for`
//! signatures - core never constructs them (contract SEC_CONSTRUCTION_SITES).
//! The lifetime of an injected secret never leaves the `open` call.

use async_trait::async_trait;

use crate::domain::{
    Capability, ConstraintSpec, CredentialTier, ResolvedTarget, ResourceCode, ResourceCredential,
};
use crate::error::{
    ClassifyError, ConstraintError, CredentialError, DiscoverError, ExecError, TransportError,
};
use crate::request::{ClassifiedIntent, Intent};

/// An established connection to a backing resource, handed back by
/// `Transport::open` and consumed by `Adapter::execute` / `Adapter::discover`
/// (detailed design step [7b] onward). Opaque carrier; the concrete payload
/// is the implementing transport's to own. Persistent channels are pooled;
/// non-persistent ones are destroyed after use (`Transport::persistent`).
pub struct Channel {
    /// Transport-private connection handle. The boxed value is whatever the
    /// implementing transport needs (a socket, a session, a pooled handle);
    /// core treats it as opaque.
    pub handle: Box<dyn Send + Sync>,
}

/// Raw, un-sanitized resource response as produced by `Adapter::execute`
/// before kernel egress (detailed design step [8] -> [9]). It MUST pass the
/// `Sanitizer` before leaving the kernel; this type is therefore never
/// serialized straight to the Agent.
pub struct RawResponse {
    /// Raw response bytes from the resource, uninterpreted by core.
    pub payload: Vec<u8>,
}

/// Control-plane discovery result: the capability surface probed from a
/// live resource (detailed design `postern:discover`; discovery is not
/// authorization). Facts only - the reachable verbs and object references,
/// never credentials or real addresses.
pub struct CapabilitySurface {
    /// Verb classes the resource exposes, as probed.
    pub capabilities: Vec<Capability>,
    /// Object references discovered on the resource (table / container /
    /// path codes), e.g. `"public.orders"`.
    pub objects: Vec<String>,
}

/// Transport plugin (detailed design 4.1; step [7b] connection underlay).
/// Implementations: ssh / ssm / direct.
#[async_trait]
pub trait Transport: Send + Sync {
    /// Transport-registry selection key (`ssh` / `ssm` / `direct`).
    fn kind(&self) -> &'static str;

    /// Whether the transport yields a long-lived channel (pooled) or a
    /// one-shot channel (destroyed after use).
    fn persistent(&self) -> bool;

    /// Opens a channel to the resolved target with the resource credential.
    ///
    /// Both secret arguments are injected by the daemon from the secrets
    /// plane; they do not implement `Clone` / `Serialize`, their `Debug` is
    /// always `REDACTED`, and their lifetime never leaves this call
    /// (contract SEC_SECRET_TYPE_DISCIPLINE). Core declares this signature
    /// but never invokes it (the secret values cannot be constructed here).
    async fn open(
        &self,
        target: ResolvedTarget,
        cred: ResourceCredential,
    ) -> Result<Channel, TransportError>;
}

/// Adapter plugin (detailed design 4.1; steps [2] [4] [8], discovery on the
/// control plane). Implementations: postgres / docker_logs / http.
#[async_trait]
pub trait Adapter: Send + Sync {
    /// Adapter-registry protocol key (`postgres` / `docker_logs` / `http`).
    fn protocol(&self) -> &'static str;

    /// Verb classes this adapter can classify intents into.
    fn capabilities(&self) -> &'static [Capability];

    /// Engine-level enforcement availability: `true` for SQL-class adapters
    /// (credential-tier fallback), `false` for HTTP/container-class adapters
    /// (classification plus constraints are the only defense).
    fn engine_enforced(&self) -> bool;

    /// Step [2]: whitelist-classify the raw intent into a verb class plus
    /// objects. `Err` denies (axiom two - whitelist classification).
    fn classify(&self, intent: &Intent) -> Result<ClassifiedIntent, ClassifyError>;

    /// Step [4]: check one object constraint against the classified intent.
    /// `Ok(false)` or `Err` denies; "cannot decide" equals "not passed".
    fn check_constraint(
        &self,
        spec: &ConstraintSpec,
        ci: &ClassifiedIntent,
    ) -> Result<bool, ConstraintError>;

    /// Step [8]: execute the intent over an open channel, returning the raw
    /// (un-sanitized) response. An already-executed request is never
    /// reported as deny (detailed design 6.1).
    async fn execute(&self, ch: &mut Channel, intent: &Intent) -> Result<RawResponse, ExecError>;

    /// Control-plane discovery: probe the live capability surface over an
    /// open channel. Discovery is not authorization.
    async fn discover(&self, ch: &mut Channel) -> Result<CapabilitySurface, DiscoverError>;
}

/// Resource-credential source (technical design 10.6). Implementations: a
/// static vault; the interface reserves room for dynamic issuance /
/// certificates.
#[async_trait]
pub trait CredentialProvider: Send + Sync {
    /// Materializes the credential for `(resource, tier)`. There is no
    /// default credential: a missing `(resource, tier)` denies at the tier
    /// stage. The returned secret is consumed once by `Transport::open` and
    /// is never constructed in core (contract SEC_CONSTRUCTION_SITES).
    async fn credential_for(
        &self,
        res: &ResourceCode,
        tier: &CredentialTier,
    ) -> Result<ResourceCredential, CredentialError>;
}
