//! Shared plan/approval contract for future system-changing operations (#4 and later).
//!
//! A [`Plan`] binds an approval to a core plan. Approving it yields a single-use
//! [`ApprovalToken`] bound to:
//! - the core plan digest (`dotfilesync_core::Plan::digest`),
//! - the owning user (UID from peer credentials, never from a request field),
//! - the observed-state fingerprint the plan was computed from,
//! - an expiry time.
//!
//! Redeeming checks all of these and consumes the token atomically, so a token can never be
//! replayed. A failed redemption also burns the token. Only `sha256(token)` is stored, in memory;
//! tokens are never journaled or logged, and a daemon restart invalidates every approval.
//!
//! Nothing in this module executes anything. Planned actions are descriptive data only.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use thiserror::Error;

use crate::fsutil::random_hex;

/// How long an approval stays redeemable.
pub const APPROVAL_TTL_MS: u64 = 10 * 60 * 1000;

/// Approval binding for a core plan.
///
/// `content_digest` is [`dotfilesync_core::Plan::digest`], not a hash of this struct. The
/// state fingerprint and owner are checked on their own at redeem time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub plan_id: String,
    /// Resource key used to supersede older approvals (the home the plan mutates).
    pub resource: String,
    /// `dotfilesync_core::Plan::digest()`.
    pub content_digest: String,
    /// Fingerprint of the observed state the plan was computed from.
    pub state_fingerprint: String,
    pub owner_uid: u32,
}

impl Plan {
    pub fn digest(&self) -> String {
        self.content_digest.clone()
    }
}

/// Opaque single-use approval token. `Debug`/`Display` never reveal it.
#[derive(Clone, PartialEq, Eq)]
pub struct ApprovalToken(String);

impl ApprovalToken {
    /// The secret value, for handing to the approving client only.
    pub fn expose(&self) -> &str {
        &self.0
    }
    pub fn from_client(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    fn hash(&self) -> String {
        hex::encode(Sha256::digest(self.0.as_bytes()))
    }
}

impl fmt::Debug for ApprovalToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApprovalToken(<redacted>)")
    }
}

impl fmt::Display for ApprovalToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted approval token>")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ApprovalError {
    #[error("approval token is unknown")]
    UnknownToken,
    #[error("approval has expired")]
    Expired,
    #[error("approval token was already used")]
    AlreadyUsed,
    #[error("plan changed after it was approved")]
    PlanMismatch,
    #[error("observed state changed after the plan was approved; re-plan and approve again")]
    StateChanged,
    #[error("approval belongs to a different user")]
    WrongOwner,
    #[error("could not generate approval token")]
    Entropy,
}

#[derive(Debug)]
struct Approval {
    resource: String,
    plan_digest: String,
    owner_uid: u32,
    state_fingerprint: String,
    expires_at_ms: u64,
    consumed: bool,
}

#[derive(Debug, Default)]
pub struct ApprovalStore {
    approvals: Mutex<HashMap<String, Approval>>,
}

impl ApprovalStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Approve `plan` on behalf of `approver_uid` (from peer credentials). The approver must
    /// own the plan.
    pub fn issue(
        &self,
        plan: &Plan,
        approver_uid: u32,
        now_ms: u64,
        ttl_ms: u64,
    ) -> Result<ApprovalToken, ApprovalError> {
        if approver_uid != plan.owner_uid {
            return Err(ApprovalError::WrongOwner);
        }
        let token = ApprovalToken(random_hex(32).map_err(|_| ApprovalError::Entropy)?);
        let mut map = self.approvals.lock().unwrap_or_else(|p| p.into_inner());
        // Drop expired and consumed entries so the store stays bounded by live approvals, and
        // supersede older approvals for the same resource: approving a new plan invalidates them.
        map.retain(|_, a| !a.consumed && a.expires_at_ms > now_ms && a.resource != plan.resource);
        map.insert(
            token.hash(),
            Approval {
                resource: plan.resource.clone(),
                plan_digest: plan.digest(),
                owner_uid: plan.owner_uid,
                state_fingerprint: plan.state_fingerprint.clone(),
                expires_at_ms: now_ms.saturating_add(ttl_ms),
                consumed: false,
            },
        );
        Ok(token)
    }

    /// Validate and consume `token` for applying `plan` against `current_state`.
    /// Any outcome other than `Ok` on a known token also invalidates it.
    pub fn redeem(
        &self,
        token: &ApprovalToken,
        plan: &Plan,
        caller_uid: u32,
        current_state: &str,
        now_ms: u64,
    ) -> Result<(), ApprovalError> {
        let mut map = self.approvals.lock().unwrap_or_else(|p| p.into_inner());
        let a = map
            .get_mut(&token.hash())
            .ok_or(ApprovalError::UnknownToken)?;
        if a.consumed {
            return Err(ApprovalError::AlreadyUsed);
        }
        a.consumed = true;
        if now_ms >= a.expires_at_ms {
            return Err(ApprovalError::Expired);
        }
        if caller_uid != a.owner_uid || plan.owner_uid != a.owner_uid {
            return Err(ApprovalError::WrongOwner);
        }
        if plan.digest() != a.plan_digest {
            return Err(ApprovalError::PlanMismatch);
        }
        if current_state != a.state_fingerprint {
            return Err(ApprovalError::StateChanged);
        }
        Ok(())
    }

    /// Invalidate outstanding approvals for `resource`, e.g. when its plan is recomputed or the
    /// resource is observed to change.
    pub fn revoke_resource(&self, resource: &str) {
        self.approvals
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|_, a| a.resource != resource);
    }

    /// Invalidate every outstanding approval.
    pub fn revoke_all(&self) {
        self.approvals
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }

    pub fn live_count(&self, now_ms: u64) -> usize {
        self.approvals
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .filter(|a| !a.consumed && a.expires_at_ms > now_ms)
            .count()
    }
}
