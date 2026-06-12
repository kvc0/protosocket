//! Safe-ish RAII wrappers over the libfabric object graph.
//!
//! libfabric objects form an ownership tree —
//! `fabric → domain → {av, cq, endpoint, memory region}` — and every object is
//! torn down with the same `fi_close` call. This module is the *only* place
//! that opens or closes libfabric objects, so the unsafe teardown logic lives
//! exactly once.
//!
//! The raw pointers are `Send`/`Sync` because we request `FI_THREAD_SAFE` from
//! the provider and confine almost all fabric use to the single
//! [`super::driver`] reaper (thread or async task).

use std::ffi::c_void;
use std::os::fd::RawFd;
use std::ptr;

use super::error::{check, EfaError};
use super::ffi::{self, fi};
use super::provider::Provider;

/// libfabric's `FI_ADDR_UNSPEC` (a C macro, so not in the generated bindings).
pub const FI_ADDR_UNSPEC: fi::fi_addr_t = u64::MAX;

/// Memory-access flags for registered buffers: we both send and receive.
const MR_ACCESS: u64 = (fi::FI_SEND | fi::FI_RECV) as u64;
/// CQ-bind flags for an endpoint: report both transmit and receive completions.
const EP_CQ_FLAGS: u64 = (fi::FI_TRANSMIT | fi::FI_RECV) as u64;

/// Close a libfabric object by pointer. All `fid_*` structs embed `struct fid`
/// as their first member, so a pointer to any of them is a valid `*mut fid`.
///
/// # Safety
/// `ptr` must be a non-null pointer to a live libfabric object that is not
/// closed elsewhere.
unsafe fn close(ptr: *mut c_void) {
    if !ptr.is_null() {
        let ret = unsafe { fi::fi_close(ptr as *mut fi::fid) };
        if ret != 0 {
            log::warn!("fi_close returned {ret}");
        }
    }
}

/// An owned `fi_info` (or chain). Freed with `fi_freeinfo`.
struct OwnedInfo(*mut fi::fi_info);

impl Drop for OwnedInfo {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: we own this allocation; freed exactly once.
            unsafe { fi::fi_freeinfo(self.0) };
        }
    }
}

/// A libfabric fabric + domain + address vector + completion queue.
///
/// One of these backs an [`super::listener::EfaSocketListener`] (server) or a
/// client. Endpoints created from it ([`Fabric::create_endpoint`]) share its
/// AV and CQ, so the reaper can service every connection by reading one CQ.
pub struct Fabric {
    info: OwnedInfo,
    fabric: *mut fi::fid_fabric,
    domain: *mut fi::fid_domain,
    av: *mut fi::fid_av,
    cq: *mut fi::fid_cq,
    /// An fd that becomes readable when the CQ has completions, when the
    /// provider supports `FI_WAIT_FD`. `None` means the CQ must be busy-polled.
    wait_fd: Option<RawFd>,
}

// SAFETY: the fabric is built with FI_THREAD_SAFE and ownership is moved to the
// driver thread; the raw pointers are never aliased across threads without that
// guarantee.
unsafe impl Send for Fabric {}

impl Fabric {
    /// Discover and open a fabric for `provider`.
    pub fn open(provider: Provider) -> Result<Self, EfaError> {
        let info = discover(provider)?;
        let raw = info.0;

        let mut fabric: *mut fi::fid_fabric = ptr::null_mut();
        // SAFETY: fabric_attr is non-null on a fi_getinfo result.
        let ret = unsafe { fi::fi_fabric((*raw).fabric_attr, &mut fabric, ptr::null_mut()) };
        check("fi_fabric", ret)?;

        let mut domain: *mut fi::fid_domain = ptr::null_mut();
        // SAFETY: fabric and info are live.
        let ret = unsafe { fi::fi_domain(fabric, raw, &mut domain, ptr::null_mut()) };
        if let Err(e) = check("fi_domain", ret) {
            unsafe { close(fabric.cast()) };
            return Err(e);
        }

        // SAFETY: av_attr is a fully-zeroed POD struct.
        let mut av_attr: fi::fi_av_attr = unsafe { std::mem::zeroed() };
        av_attr.type_ = fi::fi_av_type_FI_AV_TABLE;
        let mut av: *mut fi::fid_av = ptr::null_mut();
        // SAFETY: domain is live; av_attr is a fully-initialized local.
        let ret = unsafe { fi::fi_av_open(domain, &mut av_attr, &mut av, ptr::null_mut()) };
        if let Err(e) = check("fi_av_open", ret) {
            unsafe {
                close(domain.cast());
                close(fabric.cast());
            }
            return Err(e);
        }

        // Prefer a CQ whose readiness we can await via an fd (`FI_WAIT_FD`), so
        // the driver can integrate with tokio instead of busy-polling. Providers
        // without it (EFA, udp, shm) fall back to a busy-poll thread.
        // SAFETY: domain is live; `open_cq` closes its CQ on any failure path.
        let (cq, wait_fd) = match unsafe { open_cq(domain) } {
            Ok(pair) => pair,
            Err(e) => {
                unsafe {
                    close(av.cast());
                    close(domain.cast());
                    close(fabric.cast());
                }
                return Err(e);
            }
        };
        match wait_fd {
            Some(fd) => log::debug!("{provider:?}: awaitable CQ (FI_WAIT_FD, fd={fd})"),
            None => log::debug!("{provider:?}: busy-poll CQ (no FI_WAIT_FD)"),
        }

        Ok(Fabric {
            info,
            fabric,
            domain,
            av,
            cq,
            wait_fd,
        })
    }

    /// The shared completion queue. Read by the driver.
    pub fn cq(&self) -> *mut fi::fid_cq {
        self.cq
    }

    /// The CQ's wait fd, if the provider supports `FI_WAIT_FD`. When present the
    /// driver awaits this fd (via tokio's `AsyncFd`); when absent it busy-polls.
    pub fn wait_fd(&self) -> Option<RawFd> {
        self.wait_fd
    }

    /// Arm the CQ's wait fd for blocking, per libfabric's wait protocol.
    ///
    /// Returns `Ok(true)` when it is safe to block on the fd, or `Ok(false)`
    /// when completions are already pending and the CQ should be read again
    /// before blocking. Only meaningful when [`Fabric::wait_fd`] is `Some`.
    pub fn try_wait(&self) -> Result<bool, EfaError> {
        let mut fids: [*mut fi::fid; 1] = [self.cq.cast()];
        // SAFETY: fabric and cq are live; we pass one valid fid.
        let ret = unsafe { fi::fi_trywait(self.fabric, fids.as_mut_ptr(), 1) };
        if ret == 0 {
            Ok(true)
        } else if -ret == fi::FI_EAGAIN as i32 {
            Ok(false)
        } else {
            Err(EfaError::libfabric("fi_trywait", ret))
        }
    }

    /// Insert a peer's raw fabric address into the shared AV, returning the
    /// `fi_addr_t` handle used to address sends to that peer.
    pub fn insert_address(&self, raw_addr: &[u8]) -> Result<fi::fi_addr_t, EfaError> {
        let mut fi_addr: fi::fi_addr_t = FI_ADDR_UNSPEC;
        // SAFETY: av is live; raw_addr points to readable bytes; one address.
        let ret = unsafe {
            fi::fi_av_insert(
                self.av,
                raw_addr.as_ptr() as *const c_void,
                1,
                &mut fi_addr,
                0,
                ptr::null_mut(),
            )
        };
        // fi_av_insert returns the number of successful insertions.
        let inserted = check("fi_av_insert", ret)?;
        if inserted != 1 {
            return Err(EfaError::Bootstrap(format!(
                "fi_av_insert inserted {inserted} of 1 addresses"
            )));
        }
        Ok(fi_addr)
    }

    /// Create, bind, and enable a fresh endpoint sharing this fabric's AV and
    /// CQ. Returns the endpoint and its raw local address (to send to the peer
    /// over the bootstrap channel).
    pub fn create_endpoint(&self) -> Result<(Endpoint, Vec<u8>), EfaError> {
        let mut ep: *mut fi::fid_ep = ptr::null_mut();
        // SAFETY: domain and info are live.
        let ret = unsafe { fi::fi_endpoint(self.domain, self.info.0, &mut ep, ptr::null_mut()) };
        check("fi_endpoint", ret)?;
        let endpoint = Endpoint { ep };

        // Bind the address vector (no flags) and the completion queue.
        // SAFETY: ep, av, cq are live; av/cq are valid fids.
        let ret = unsafe { fi::fi_ep_bind(ep, self.av.cast(), 0) };
        check("fi_ep_bind(av)", ret)?;
        // SAFETY: as above.
        let ret = unsafe { fi::fi_ep_bind(ep, self.cq.cast(), EP_CQ_FLAGS) };
        check("fi_ep_bind(cq)", ret)?;

        // SAFETY: ep is live and fully bound.
        let ret = unsafe { fi::fi_enable(ep) };
        check("fi_enable", ret)?;

        let address = endpoint.local_address()?;
        Ok((endpoint, address))
    }

    /// Register `buffer` for fabric send/recv, returning its descriptor handle.
    pub fn register(&self, buffer: &[u8]) -> Result<MemoryRegion, EfaError> {
        let mut mr: *mut fi::fid_mr = ptr::null_mut();
        // SAFETY: domain is live; buffer outlives the MemoryRegion (the caller
        // keeps the backing allocation alive).
        let ret = unsafe {
            fi::fi_mr_reg(
                self.domain,
                buffer.as_ptr() as *const c_void,
                buffer.len(),
                MR_ACCESS,
                0,
                0,
                0,
                &mut mr,
                ptr::null_mut(),
            )
        };
        check("fi_mr_reg", ret)?;
        // SAFETY: mr is live.
        let desc = unsafe { fi::fi_mr_desc(mr) };
        Ok(MemoryRegion { mr, desc })
    }
}

impl Drop for Fabric {
    fn drop(&mut self) {
        // Close in reverse dependency order. Endpoints are owned separately and
        // must already be dropped.
        // SAFETY: each pointer was opened by this Fabric and is closed once.
        unsafe {
            close(self.cq.cast());
            close(self.av.cast());
            close(self.domain.cast());
            close(self.fabric.cast());
        }
        // `info` is freed by its own Drop after this.
    }
}

/// An enabled endpoint. Each connection owns exactly one.
pub struct Endpoint {
    ep: *mut fi::fid_ep,
}

// SAFETY: see `Fabric` — confined to the driver thread / FI_THREAD_SAFE.
unsafe impl Send for Endpoint {}

impl Endpoint {
    /// The raw pointer, for posting sends/recvs from [`super::driver`].
    pub fn as_ptr(&self) -> *mut fi::fid_ep {
        self.ep
    }

    /// This endpoint's raw local fabric address.
    fn local_address(&self) -> Result<Vec<u8>, EfaError> {
        // First call with zero length to learn the address size.
        let mut len: usize = 0;
        // SAFETY: ep is live; a null buffer with len 0 asks libfabric for the
        // required length (returned via -FI_ETOOSMALL).
        unsafe { fi::fi_getname(self.ep.cast(), ptr::null_mut(), &mut len) };
        if len == 0 {
            return Err(EfaError::Bootstrap(
                "fi_getname reported a zero-length address".to_string(),
            ));
        }
        let mut addr = vec![0u8; len];
        // SAFETY: ep is live; addr has `len` writable bytes.
        let ret =
            unsafe { fi::fi_getname(self.ep.cast(), addr.as_mut_ptr() as *mut c_void, &mut len) };
        check("fi_getname", ret)?;
        addr.truncate(len);
        Ok(addr)
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        // SAFETY: ep was opened by create_endpoint and is closed once here.
        unsafe { close(self.ep.cast()) };
    }
}

/// A registered memory region. The backing bytes are owned by the caller and
/// must outlive this handle.
pub struct MemoryRegion {
    mr: *mut fi::fid_mr,
    desc: *mut c_void,
}

// SAFETY: see `Fabric`.
unsafe impl Send for MemoryRegion {}

impl MemoryRegion {
    /// The local descriptor to pass to `fi_send`/`fi_recv` for this region.
    pub fn desc(&self) -> *mut c_void {
        self.desc
    }
}

impl Drop for MemoryRegion {
    fn drop(&mut self) {
        // SAFETY: mr was registered by `Fabric::register` and is closed once.
        unsafe { close(self.mr.cast()) };
    }
}

/// Open the completion queue, preferring an awaitable (`FI_WAIT_FD`) one.
///
/// Returns the CQ and its wait fd. Falls back to `FI_WAIT_NONE` (and `None`) if
/// the provider can't open an fd-wait CQ (EFA, shm) or can't hand back the fd
/// (udp). On its own error paths it closes any CQ it opened.
///
/// # Safety
/// `domain` must be a live domain.
unsafe fn open_cq(
    domain: *mut fi::fid_domain,
) -> Result<(*mut fi::fid_cq, Option<RawFd>), EfaError> {
    if let Ok(cq) = open_cq_raw(domain, fi::fi_wait_obj_FI_WAIT_FD) {
        match get_wait_fd(cq) {
            Some(fd) => return Ok((cq, Some(fd))),
            // Opened an fd-wait CQ but the provider won't expose the fd (udp):
            // unusable, so discard it and fall back.
            None => close(cq.cast()),
        }
    }
    let cq = open_cq_raw(domain, fi::fi_wait_obj_FI_WAIT_NONE)?;
    Ok((cq, None))
}

/// Open a CQ with the given wait object and our message completion format.
///
/// # Safety
/// `domain` must be a live domain.
unsafe fn open_cq_raw(
    domain: *mut fi::fid_domain,
    wait_obj: fi::fi_wait_obj,
) -> Result<*mut fi::fid_cq, EfaError> {
    let mut cq_attr: fi::fi_cq_attr = std::mem::zeroed();
    // FI_CQ_FORMAT_MSG carries op_context + flags + len, which is all the driver
    // needs to route completions (each endpoint has a single peer).
    cq_attr.format = fi::fi_cq_format_FI_CQ_FORMAT_MSG;
    cq_attr.wait_obj = wait_obj;
    let mut cq: *mut fi::fid_cq = ptr::null_mut();
    check(
        "fi_cq_open",
        fi::fi_cq_open(domain, &mut cq_attr, &mut cq, ptr::null_mut()),
    )?;
    Ok(cq)
}

/// Extract a CQ's wait fd via `fi_control(FI_GETWAIT)`, if available.
///
/// # Safety
/// `cq` must be a live completion queue.
unsafe fn get_wait_fd(cq: *mut fi::fid_cq) -> Option<RawFd> {
    let mut fd: RawFd = -1;
    let ret = fi::fi_control(
        cq.cast(),
        fi::FI_GETWAIT as std::os::raw::c_int,
        &mut fd as *mut RawFd as *mut c_void,
    );
    if ret == 0 && fd >= 0 {
        Some(fd)
    } else {
        None
    }
}

/// Run `fi_getinfo` for `provider`, requesting a reliable-datagram endpoint with
/// message semantics and thread-safe operation.
fn discover(provider: Provider) -> Result<OwnedInfo, EfaError> {
    // SAFETY: fi_allocinfo returns a zeroed fi_info with sub-attributes
    // allocated, or null on OOM.
    let hints = unsafe { fi::fi_allocinfo() };
    if hints.is_null() {
        return Err(EfaError::Bootstrap(
            "fi_allocinfo returned null".to_string(),
        ));
    }
    // Ensure hints is freed on every path.
    let hints = OwnedInfo(hints);

    // SAFETY: hints and its sub-attributes are non-null (just allocated).
    unsafe {
        let h = hints.0;
        (*h).caps = fi::FI_MSG as u64;
        (*(*h).ep_attr).type_ = fi::fi_ep_type_FI_EP_RDM;
        (*(*h).domain_attr).threading = fi::fi_threading_FI_THREAD_SAFE;
        (*(*h).domain_attr).av_type = fi::fi_av_type_FI_AV_TABLE;
        // Advertise the MR responsibilities we satisfy (we always register
        // local buffers and pass descriptors). Providers needing none of these
        // — e.g. tcp/udp — simply ignore the hint.
        (*(*h).domain_attr).mr_mode = (fi::FI_MR_LOCAL
            | fi::FI_MR_VIRT_ADDR
            | fi::FI_MR_ALLOCATED
            | fi::FI_MR_PROV_KEY
            | fi::FI_MR_ENDPOINT) as i32;
        // Select the provider by name (libfabric frees this with `free`).
        (*(*h).fabric_attr).prov_name = libc_strdup(provider.prov_name_cstr().to_bytes_with_nul());
    }

    let mut info: *mut fi::fi_info = ptr::null_mut();
    // SAFETY: hints is a valid fi_info; info receives the result chain.
    let ret = unsafe {
        fi::fi_getinfo(
            ffi::api_version(),
            ptr::null(),
            ptr::null(),
            0,
            hints.0,
            &mut info,
        )
    };
    if ret != 0 {
        if -ret == fi::FI_ENODATA as i32 {
            return Err(EfaError::ProviderUnavailable { provider });
        }
        return Err(EfaError::libfabric("fi_getinfo", ret));
    }
    if info.is_null() {
        return Err(EfaError::ProviderUnavailable { provider });
    }
    Ok(OwnedInfo(info))
}

/// Duplicate `bytes` (which include a trailing NUL) into a libfabric-owned
/// allocation via `malloc`, so `fi_freeinfo` can `free` it.
fn libc_strdup(bytes_with_nul: &[u8]) -> *mut std::os::raw::c_char {
    let len = bytes_with_nul.len();
    // SAFETY: malloc of `len` bytes; we copy exactly `len` bytes (incl. NUL).
    unsafe {
        let p = malloc(len) as *mut u8;
        assert!(!p.is_null(), "malloc failed for provider name");
        ptr::copy_nonoverlapping(bytes_with_nul.as_ptr(), p, len);
        p as *mut std::os::raw::c_char
    }
}

unsafe extern "C" {
    fn malloc(size: usize) -> *mut c_void;
}

#[cfg(test)]
mod waitfd_probe {
    //! Empirically determine which providers support an `FI_WAIT_FD` completion
    //! queue (an fd we can await with tokio's `AsyncFd`). Run with:
    //! `cargo test -p protosocket-efa --lib waitfd -- --ignored --nocapture`

    use super::*;

    /// Open a fabric for `provider` and report whether its CQ offers a wait fd,
    /// reusing the same `open_cq` path the real `Fabric` uses.
    fn probe(provider: Provider) -> Result<Option<RawFd>, EfaError> {
        let fabric = Fabric::open(provider)?;
        Ok(fabric.wait_fd())
    }

    #[test]
    #[ignore = "probes local libfabric providers; run manually"]
    fn report_waitfd_support() {
        for provider in [
            Provider::Tcp,
            Provider::Udp,
            Provider::Shm,
            Provider::Verbs,
            Provider::Efa,
        ] {
            match probe(provider) {
                Ok(Some(fd)) => println!("{provider:?}: FI_WAIT_FD OK, fd={fd}"),
                Ok(None) => println!("{provider:?}: no FI_WAIT_FD (busy-poll)"),
                Err(e) => println!("{provider:?}: unavailable ({e})"),
            }
        }
    }
}
