//! Thin, centralized access to the raw libfabric FFI.
//!
//! All other modules go through this one so that the (unsafe) surface against
//! `ofi-libfabric-sys` lives in a single place. Re-export the generated
//! bindings as [`fi`] and provide a couple of small helpers that the C `static
//! inline` macros would otherwise give us.

pub use ofi_libfabric_sys::bindgen as fi;

/// The libfabric API version this crate is built against.
///
/// libfabric exposes `FI_VERSION(major, minor)` as a C macro that bindgen does
/// not translate. The runtime `fi_version()` returns the version of the loaded
/// library, which is the correct value to request in `fi_getinfo`.
pub fn api_version() -> u32 {
    // SAFETY: `fi_version` takes no arguments and only reads constants.
    unsafe { fi::fi_version() }
}
