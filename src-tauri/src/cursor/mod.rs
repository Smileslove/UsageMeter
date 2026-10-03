//! Cursor account usage. No conversation bodies or credentials are persisted here.
pub(crate) mod client;
pub(crate) mod events;
pub(crate) mod sync;

// P0 requires a real account sample before enabling the private dashboard contract.
// Reference snapshots and synthetic fixtures do not establish live compatibility.
pub(crate) const ACCOUNT_API_CONTRACT_VERIFIED: bool = false;
