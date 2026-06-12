//! Error type for the libfabric-backed listener.

use std::ffi::{c_int, CStr};

use super::ffi::fi;

/// Errors produced while setting up or operating an EFA fabric.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EfaError {
    /// A libfabric call returned a negative error code. `call` is the name of
    /// the libfabric function and `message` is `fi_strerror`'s description.
    #[error("libfabric call {call} failed ({code}): {message}")]
    Libfabric {
        /// The libfabric function that failed (e.g. `"fi_getinfo"`).
        call: &'static str,
        /// The (positive) libfabric error code.
        code: i32,
        /// Human-readable description from `fi_strerror`.
        message: String,
    },

    /// No fabric matched the requested provider on this host.
    #[error("no libfabric provider matched {provider:?}")]
    ProviderUnavailable {
        /// The provider that was requested.
        provider: crate::Provider,
    },

    /// The TCP side-channel handshake failed.
    #[error("connection bootstrap failed: {0}")]
    Bootstrap(String),

    /// An underlying I/O error (TCP side-channel, etc.).
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl EfaError {
    /// Build a [`EfaError::Libfabric`] from a libfabric return value.
    ///
    /// libfabric returns `0` on success and a negative `-errno`-style code on
    /// failure; `code` here is the negated (positive) value.
    pub(crate) fn libfabric(call: &'static str, ret: c_int) -> Self {
        let code = -ret;
        // SAFETY: `fi_strerror` returns a pointer to a static, NUL-terminated
        // string owned by libfabric. We copy it out immediately.
        let message = unsafe {
            let ptr = fi::fi_strerror(code);
            if ptr.is_null() {
                String::from("unknown error")
            } else {
                CStr::from_ptr(ptr).to_string_lossy().into_owned()
            }
        };
        EfaError::Libfabric {
            call,
            code,
            message,
        }
    }
}

/// Convert a libfabric return value into a `Result`, attaching the call name.
///
/// Returns `Ok(ret)` when `ret >= 0` (many libfabric calls return a count),
/// and `Err` otherwise.
pub(crate) fn check(call: &'static str, ret: c_int) -> Result<c_int, EfaError> {
    if ret >= 0 {
        Ok(ret)
    } else {
        Err(EfaError::libfabric(call, ret))
    }
}

impl From<EfaError> for std::io::Error {
    fn from(error: EfaError) -> Self {
        match error {
            EfaError::Io(io) => io,
            other => std::io::Error::other(other),
        }
    }
}
