//! Se — Security Reference Monitor stub.
//!
//! In Windows NT the SRM sits between the Object Manager and every access check.
//! This stub always grants access; a real implementation would validate
//! `AccessToken` privileges against the requested `DesiredAccess` mask.

pub mod token;

/// Check whether the token `token` grants `desired_access` to an object
/// protected by `security_descriptor`.
///
/// Always returns `true` for now.
pub fn se_access_check(desired_access: u32, _token: *const token::AccessToken) -> bool {
    log::trace!(
        "SeAccessCheck: desired_access={:#x} (stub — granted)",
        desired_access
    );
    true
}
