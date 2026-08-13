//! POSIX shared-memory frame handoff for the Kitty graphics protocol (`t=s`).
//!
//! # Why this exists
//!
//! Every other way of getting a frame to a terminal emulator puts the *pixels*
//! on the wire. Even after palette-indexing and deflating, a frame is a couple
//! of kilobytes of base64 that the terminal must scan byte by byte, decode, and
//! upload — sixty times a second, forever.
//!
//! The Kitty graphics protocol has a transmission medium that skips the wire
//! entirely. With `t=s` the client writes the raw pixels into a POSIX shared
//! memory object and sends only its **name**:
//!
//! ```text
//! \x1b_Ga=T,i=1,f=24,t=s,s=160,v=144,c=122,r=55;L2diLTEyMy00\x1b\
//! ```
//!
//! That is about **sixty bytes**, independent of the picture. The terminal
//! `shm_open`s the object, `mmap`s it, uploads the texture straight from that
//! mapping and unlinks it. No base64, no decode, no parser work proportional to
//! the image — the two processes look at the same physical pages.
//!
//! At 60 Hz that is ~3.6 KB/s of terminal input instead of the 5.5 MB/s that had
//! WezTerm rendering at 6 fps. See [`docs/rendering.md`](../docs/rendering.md).
//!
//! # The lifetime rule that shapes this code
//!
//! > "The terminal emulator will delete the shared memory object after reading
//! > the data from it."
//! > — [kitty graphics protocol, *Transferring pixel data*](https://sw.kovidgoyal.net/kitty/graphics-protocol/#transferring-pixel-data)
//!
//! So an object **cannot be reused across frames**: by the time the next frame
//! is ready the terminal has unlinked the last one. Each frame therefore creates
//! a fresh object under a fresh name. The cost is three syscalls and a memcpy
//! (measured at a few tens of microseconds against a 16.7 ms frame), which is
//! the trade that buys away the entire wire cost.
//!
//! Because the terminal does the unlinking, a name can leak in `/dev/shm` if the
//! terminal ignores the escape (no `t=s` support, or the sequence was swallowed
//! by a multiplexer). [`ShmFrame`] therefore unlinks on drop as well — the
//! unlink is idempotent, so both sides trying is correct and neither leaks.
//!
//! # Availability
//!
//! Unix only, and only where `/dev/shm` (or the platform's `shm_open` backing
//! store) is writable — so not over SSH to a terminal on another machine, not in
//! a container whose `/dev/shm` the terminal cannot see, and not on `wasm32`.
//! Every one of those cases falls back to the compressed-PNG transport, and the
//! frontend decides which to use by *asking the terminal*, never by guessing:
//! see [`crate::termimage::TermImage::set_kitty_shm`].
//!
//! # Platform gating
//!
//! The whole module compiles everywhere. Where shared memory is not available
//! (`wasm32`, non-Unix, or a build without the `libc` feature) [`ShmFrame::new`]
//! is a stub returning `None`, and every caller already treats `None` as "use
//! another transport". That keeps the `cfg` noise in this one file instead of
//! spreading it through the encoder and the frontend.

/// True when this build can actually create shared-memory frames.
pub const AVAILABLE: bool =
    cfg!(all(unix, feature = "libc", not(target_arch = "wasm32")));

#[cfg(all(unix, feature = "libc", not(target_arch = "wasm32")))]
pub use imp::ShmFrame;

#[cfg(not(all(unix, feature = "libc", not(target_arch = "wasm32"))))]
pub use stub::ShmFrame;

/// The no-shared-memory platforms: same surface, always unavailable.
#[cfg(not(all(unix, feature = "libc", not(target_arch = "wasm32"))))]
mod stub {
    pub struct ShmFrame(());

    impl ShmFrame {
        pub fn new(_bytes: usize, _fill: impl FnOnce(&mut [u8])) -> Option<ShmFrame> {
            None
        }
        pub fn name(&self) -> &str {
            ""
        }
        pub fn len(&self) -> usize {
            0
        }
        pub fn is_empty(&self) -> bool {
            true
        }
        pub fn unlink(&mut self) {}
    }
}

#[cfg(all(unix, feature = "libc", not(target_arch = "wasm32")))]
mod imp {
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A shared-memory object holding one frame, unlinked when dropped.
    pub struct ShmFrame {
        /// The name to hand the terminal, e.g. `/gb-4213-7`. Includes the leading
        /// slash, which `shm_open` requires and the protocol expects verbatim.
        name: String,
        ptr: *mut libc::c_void,
        len: usize,
        /// Cleared once the object has been handed over, so a later terminal-side
        /// unlink is not raced by ours.
        unlinked: bool,
    }

    // The mapping is owned exclusively by this struct and never aliased.
    unsafe impl Send for ShmFrame {}

    /// Distinguishes objects created by one process within one second.
    static SEQ: AtomicU32 = AtomicU32::new(0);

    /// Where the platform keeps POSIX shared memory. Only used for the cleanup
    /// sweep; creating and opening objects goes through `shm_open`, which is the
    /// portable interface.
    const SHM_DIR: &str = "/dev/shm";

    static SWEPT: std::sync::Once = std::sync::Once::new();

    /// Remove frame objects left behind by processes that are gone.
    ///
    /// **A `Drop` impl is not a cleanup guarantee** — this repo learned that the
    /// hard way with battery saves (see `CLAUDE.md`). `panic = 'abort'`, `SIGKILL`,
    /// a crash and a closed terminal window all skip destructors, and each skipped
    /// destructor leaves a 92 KB object in `/dev/shm` forever. So every process
    /// sweeps up after the dead ones when it starts using shared memory: parse the
    /// pid out of each `gb-<pid>-<n>` name and unlink it if no such process exists.
    ///
    /// `kill(pid, 0)` is the liveness test; it cannot tell us the pid was not
    /// recycled by an unrelated process, but the cost of that is leaving one object
    /// in place until the next sweep, which is harmless.
    fn sweep_once() {
        SWEPT.call_once(sweep_body);
    }

    /// Does this pid exist?
    ///
    /// `kill(pid, 0)` returning 0 means yes, but a **failure is not a no**: signalling
    /// a process owned by another user fails with `EPERM` while the process is
    /// plainly alive. Only `ESRCH` means "no such process". Getting this wrong makes
    /// the sweep delete objects out from under a running emulator, whose next frame
    /// then shows nothing.
    fn pid_alive(pid: u32) -> bool {
        if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }

    fn sweep_body() {
        {
            let Ok(entries) = std::fs::read_dir(SHM_DIR) else {
                return;
            };
            let me = std::process::id();
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                let Some(rest) = name.strip_prefix("gb-") else {
                    continue;
                };
                let Some((pid, _)) = rest.split_once('-') else {
                    continue;
                };
                let Ok(pid) = pid.parse::<u32>() else {
                    continue;
                };
                if pid == me || pid_alive(pid) {
                    continue; // still running — not ours to remove
                }
                if let Ok(c) = std::ffi::CString::new(format!("/{name}")) {
                    unsafe { libc::shm_unlink(c.as_ptr()) };
                }
            }
        }
    }

    impl ShmFrame {
        /// Create a shared-memory object of `bytes` bytes and fill it by calling
        /// `fill` with the mapping.
        ///
        /// Returns `None` if the platform will not give us shared memory — which is
        /// a normal, expected outcome (a container, a read-only `/dev/shm`), not an
        /// error worth reporting to the user.
        pub fn new(bytes: usize, fill: impl FnOnce(&mut [u8])) -> Option<ShmFrame> {
            if bytes == 0 {
                return None;
            }
            sweep_once();
            // A fresh name per frame: the terminal unlinks after reading, so reuse
            // would race the terminal's own cleanup.
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let name = format!("/gb-{}-{}", std::process::id(), seq);
            let c_name = std::ffi::CString::new(name.as_str()).ok()?;

            let fd = unsafe {
                libc::shm_open(
                    c_name.as_ptr(),
                    libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
                    0o600,
                )
            };
            if fd < 0 {
                return None;
            }
            if unsafe { libc::ftruncate(fd, bytes as libc::off_t) } != 0 {
                unsafe {
                    libc::close(fd);
                    libc::shm_unlink(c_name.as_ptr());
                }
                return None;
            }
            let ptr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    bytes,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd,
                    0,
                )
            };
            // The mapping keeps the object alive; the descriptor is not needed.
            unsafe { libc::close(fd) };
            if ptr == libc::MAP_FAILED {
                unsafe { libc::shm_unlink(c_name.as_ptr()) };
                return None;
            }

            let mut frame = ShmFrame {
                name,
                ptr,
                len: bytes,
                unlinked: false,
            };
            fill(frame.as_mut_slice());
            Some(frame)
        }

        pub fn name(&self) -> &str {
            &self.name
        }

        pub fn len(&self) -> usize {
            self.len
        }

        pub fn is_empty(&self) -> bool {
            self.len == 0
        }

        fn as_mut_slice(&mut self) -> &mut [u8] {
            // Safe: `ptr` is a live MAP_SHARED mapping of exactly `len` bytes, and
            // `&mut self` proves no other reference into it exists.
            unsafe { std::slice::from_raw_parts_mut(self.ptr as *mut u8, self.len) }
        }

        /// Unlink the name now, keeping the mapping alive.
        ///
        /// Use this when the terminal is **not** going to read the object (a
        /// probe that failed, a frame we decided not to send), so the name does not
        /// linger in `/dev/shm`.
        pub fn unlink(&mut self) {
            if !self.unlinked {
                if let Ok(c) = std::ffi::CString::new(self.name.as_str()) {
                    unsafe { libc::shm_unlink(c.as_ptr()) };
                }
                self.unlinked = true;
            }
        }
    }

    impl Drop for ShmFrame {
        fn drop(&mut self) {
            // Belt and braces: the terminal is supposed to unlink after reading, but
            // if the escape never reached one the name would leak. `shm_unlink` of
            // an already-unlinked name simply fails with ENOENT.
            self.unlink();
            unsafe { libc::munmap(self.ptr, self.len) };
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_frame_is_visible_under_its_name_and_unlinked_on_drop() {
            let payload: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
            let name = {
                let f = ShmFrame::new(payload.len(), |m| m.copy_from_slice(&payload))
                    .expect("shared memory unavailable on this box");
                assert_eq!(f.len(), payload.len());
                assert!(f.name().starts_with('/'), "shm_open needs a leading slash");

                // Open it the way a terminal would and check the bytes arrived.
                let c = std::ffi::CString::new(f.name()).unwrap();
                let fd = unsafe { libc::shm_open(c.as_ptr(), libc::O_RDONLY, 0) };
                assert!(fd >= 0, "terminal-side shm_open failed");
                let p = unsafe {
                    libc::mmap(
                        std::ptr::null_mut(),
                        payload.len(),
                        libc::PROT_READ,
                        libc::MAP_SHARED,
                        fd,
                        0,
                    )
                };
                unsafe { libc::close(fd) };
                assert_ne!(p, libc::MAP_FAILED);
                let seen =
                    unsafe { std::slice::from_raw_parts(p as *const u8, payload.len()) };
                assert_eq!(seen, &payload[..], "shared mapping holds the frame");
                unsafe { libc::munmap(p, payload.len()) };
                f.name().to_string()
            };

            // Dropped — the name must be gone, or every frame leaks a /dev/shm entry.
            let c = std::ffi::CString::new(name.as_str()).unwrap();
            let fd = unsafe { libc::shm_open(c.as_ptr(), libc::O_RDONLY, 0) };
            assert!(fd < 0, "{name} still exists after drop");
        }

        #[test]
        fn names_are_unique_so_consecutive_frames_never_collide() {
            // O_EXCL means a repeated name would fail outright; the terminal also
            // unlinks asynchronously, so uniqueness is the only safe design.
            let a = ShmFrame::new(16, |_| {}).expect("shm");
            let b = ShmFrame::new(16, |_| {}).expect("shm");
            assert_ne!(a.name(), b.name());
        }

        #[test]
        fn a_dead_processs_frames_are_swept_up_but_a_live_one_is_left_alone() {
            // Objects outlive a SIGKILLed process, so a fresh process must clean up
            // after the dead. Fake both cases by name: pid 1 is always alive (init),
            // and a pid this high does not exist.
            let dead = "/gb-4294967294-0";
            let live = "/gb-1-0";
            for n in [dead, live] {
                let c = std::ffi::CString::new(n).unwrap();
                let fd = unsafe {
                    libc::shm_open(c.as_ptr(), libc::O_CREAT | libc::O_RDWR, 0o600)
                };
                assert!(fd >= 0, "could not stage {n}");
                unsafe { libc::close(fd) };
            }

            // `sweep_once` is a `Once`, and `ShmFrame::new` may already have run it
            // in this test binary — call the body directly.
            SWEPT.call_once(|| {});
            sweep_body();

            let exists = |n: &str| {
                let c = std::ffi::CString::new(n).unwrap();
                let fd = unsafe { libc::shm_open(c.as_ptr(), libc::O_RDONLY, 0) };
                if fd >= 0 {
                    unsafe { libc::close(fd) };
                    true
                } else {
                    false
                }
            };
            let dead_gone = !exists(dead);
            let live_kept = exists(live);
            for n in [dead, live] {
                let c = std::ffi::CString::new(n).unwrap();
                unsafe { libc::shm_unlink(c.as_ptr()) };
            }
            assert!(dead_gone, "a dead process's frame was not swept up");
            assert!(live_kept, "swept up a frame belonging to a live process");
        }

        #[test]
        fn unlink_is_idempotent() {
            // Both we and the terminal unlink; neither may fail loudly.
            let mut f = ShmFrame::new(16, |_| {}).expect("shm");
            f.unlink();
            f.unlink();
        }
    }
}
