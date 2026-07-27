//! Remote read-only observation.
//!
//! Lets a second machine watch this host's radar. The design is deliberately staged so
//! that the part which decides WHAT may leave the machine exists and is tested before the
//! part that can actually send it: shipping a transport first would mean every later
//! redaction is a patch on a leak that already shipped.
//!
//! * [`projection`] is the redaction boundary. A remote observer only ever receives an
//!   [`projection::ObservedState`], built by the single function
//!   [`projection::project_state`]. See that module for why this is an allowlist
//!   projection rather than a scrubber.
//!
//! * [`grants`] is the credential layer: the token wire format, the verifier the host
//!   stores in place of the secret, and the single atomic statement that makes a token
//!   single use.
//!
//! * [`transport`] is the iroh endpoint and the wire protocol. It is handed a closure that
//!   yields an already-projected frame, so it has no way to name the radar's own types.
//!
//! The host-side preview command renders exactly the bytes an observer would receive,
//! through the same [`projection::project_state`] the wire uses, so the host can inspect
//! them before sharing anything.

pub mod grants;
pub mod peers;
pub mod projection;
pub mod transport;

pub use grants::{GrantRecord, GrantState, GrantStore, ObserverToken, RedeemOutcome, TokenError};
pub use peers::{ObserverHub, PeerRow};
pub use projection::{project_state, ObservedState, Profile};
pub use transport::{
    BindMode, EventSink, FrameSource, HostIdentity, HostShare, ObserveEvent, PendingApproval,
    SharingStatus,
};
