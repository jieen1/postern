//! Opaque secret-family declarations (module design 3.1/7; detailed design
//! 7.1 secret-type discipline, 7.5 log red line).
//!
//! Three types, one discipline: none of them implements `Clone`, serde
//! traits, `Display` or `Default`; `Debug` is hand-written and always
//! prints `REDACTED`, so a tracing field or format string can never leak a
//! payload - the red line is a compile-time fact, not a runtime convention.
//!
//! Construction rights differ (contract SEC_CONSTRUCTION_SITES):
//! - the resolved-target and resource-credential types have ZERO
//!   construction points in this crate: no constructor fn exists on
//!   purpose, and their payload fields are `#[doc(hidden)] pub` solely so
//!   the secrets crate can write the struct literal;
//! - `PresentedCredential` is shell-constructed (step [0] boxing), so it
//!   exposes a public constructor (and core tests may build one).
//!
//! Textual discipline for this file: the open brace of the two
//! zero-construction types (and of their impl blocks) sits on its own line
//! behind a trailing comment, so this crate never contains even the text
//! of a struct literal for them - the construction-sites scanner matches
//! the literal text "name, space, brace".

use std::fmt;

/// Gateway credential (or local-process context) as presented by the
/// Agent - step [0] boxes it, step [1] authenticators consume it.
pub struct PresentedCredential {
    kind: String,
    secret: Vec<u8>,
}

impl PresentedCredential {
    /// Boxes the presented authenticator kind plus secret bytes (shell
    /// layer, step [0]).
    pub fn new(kind: impl Into<String>, secret: Vec<u8>) -> Self {
        Self {
            kind: kind.into(),
            secret,
        }
    }

    /// Authenticator-registry selection key (step [1] picks by kind).
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Secret bytes; consumed only by authenticator implementations
    /// (hash comparison against the snapshot's `secret_hash`).
    pub fn secret(&self) -> &[u8] {
        &self.secret
    }
}

impl fmt::Debug for PresentedCredential {
    /// Always `REDACTED`, independent of the payload.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("REDACTED")
    }
}

/// Real resolved address of a resource - opaque in this crate. The secrets
/// crate is the sole producer; the transport's `open` call is the whole
/// consumer lifetime (the value never travels further).
pub struct ResolvedTarget // zero construction points in this crate
{
    /// Resolved endpoint payload. Hidden from docs; `pub` solely for the
    /// secrets-crate struct literal.
    #[doc(hidden)]
    pub endpoint: String,
}

impl fmt::Debug for ResolvedTarget // hand-written; never derived
{
    /// Always `REDACTED`, independent of the payload.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("REDACTED")
    }
}

/// Resource credential material - opaque in this crate. Produced by the
/// secrets crate (`CredentialProvider`), consumed once by the transport's
/// `open`; never pooled, logged or serialized.
pub struct ResourceCredential // zero construction points in this crate
{
    /// Credential material payload. Hidden from docs; `pub` solely for the
    /// secrets-crate struct literal.
    #[doc(hidden)]
    pub material: String,
}

impl fmt::Debug for ResourceCredential // hand-written; never derived
{
    /// Always `REDACTED`, independent of the payload.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("REDACTED")
    }
}




