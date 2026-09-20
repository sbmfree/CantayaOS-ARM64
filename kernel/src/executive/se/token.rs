//! AccessToken — represents the security context of a thread/process.

/// Integrity level (maps to NT Integrity Levels).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u32)]
pub enum IntegrityLevel {
    Untrusted = 0x0000,
    Low = 0x1000,
    Medium = 0x2000,
    High = 0x3000,
    System = 0x4000,
}

/// Stub privilege set — bitmap of granted privileges.
#[derive(Debug, Clone, Copy)]
pub struct Privileges(pub u64);

impl Privileges {
    pub const SE_DEBUG: Privileges = Privileges(1 << 0);
    pub const SE_SHUTDOWN: Privileges = Privileges(1 << 1);
    pub const SE_TCB: Privileges = Privileges(1 << 7);
    pub const SYSTEM_ALL: Privileges = Privileges(u64::MAX);

    pub fn has(self, p: Privileges) -> bool {
        self.0 & p.0 == p.0
    }
}

/// Kernel access token.
#[derive(Debug)]
pub struct AccessToken {
    pub integrity: IntegrityLevel,
    pub privileges: Privileges,
    /// SID stub (plain u64 for now; will be a proper SID structure later).
    pub user_sid: u64,
}

impl AccessToken {
    /// Token for a kernel-mode system thread.
    pub const SYSTEM_TOKEN: AccessToken = AccessToken {
        integrity: IntegrityLevel::System,
        privileges: Privileges::SYSTEM_ALL,
        user_sid: 0x0000_0000_0000_0012, // SYSTEM SID value
    };
}
