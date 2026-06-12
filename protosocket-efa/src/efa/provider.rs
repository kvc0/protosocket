//! Selection of the libfabric provider that backs the fabric.

use std::ffi::CStr;

/// The libfabric provider to use for a fabric.
///
/// A provider is libfabric's term for a backend transport implementation. EFA
/// is the reason this crate exists, but the others are useful for development
/// and testing: `Tcp`, `Udp`, and `Shm` require no special hardware, so the
/// full stack can be exercised on an ordinary machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Provider {
    /// AWS Elastic Fabric Adapter — RDMA on supported EC2 instances.
    Efa,
    /// The `tcp` provider. Works anywhere; useful for development and CI.
    Tcp,
    /// The `udp` provider. Works anywhere; useful for development and CI.
    Udp,
    /// The `verbs` provider — InfiniBand / RoCE via libibverbs.
    Verbs,
    /// The `shm` provider — intra-node shared memory.
    Shm,
}

impl Provider {
    /// The libfabric provider name, matched against `fi_info`'s
    /// `fabric_attr->prov_name` when selecting hardware.
    pub const fn prov_name(self) -> &'static str {
        match self {
            Provider::Efa => "efa",
            Provider::Tcp => "tcp",
            Provider::Udp => "udp",
            Provider::Verbs => "verbs",
            Provider::Shm => "shm",
        }
    }

    /// The provider name as a NUL-terminated C string, ready to hand to
    /// `fi_getinfo` hints.
    pub const fn prov_name_cstr(self) -> &'static CStr {
        // These are compile-time constants; each literal includes its NUL.
        match self {
            Provider::Efa => c"efa",
            Provider::Tcp => c"tcp",
            Provider::Udp => c"udp",
            Provider::Verbs => c"verbs",
            Provider::Shm => c"shm",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Provider;

    #[test]
    fn prov_name_matches_cstr() {
        for provider in [
            Provider::Efa,
            Provider::Tcp,
            Provider::Udp,
            Provider::Verbs,
            Provider::Shm,
        ] {
            assert_eq!(
                provider.prov_name(),
                provider.prov_name_cstr().to_str().expect("valid utf8"),
                "the &str and CStr provider names must agree",
            );
        }
    }

    #[test]
    fn efa_is_named_efa() {
        assert_eq!(Provider::Efa.prov_name(), "efa");
    }
}
