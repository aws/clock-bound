#![expect(clippy::cast_ptr_alignment, reason = "TODO COME BACK TO THIS")]

use errno::{Errno, errno};
use std::ffi::{CStr, c_void};
use std::mem::size_of;
use std::ptr;
use std::sync::atomic::{self, AtomicU16};
use std::sync::{Mutex, PoisonError};

use crate::shm::shm_header::{CLOCKBOUND_SHM_LATEST_VERSION, ShmHeader};
use crate::{
    shm::{
        ClockErrorBound, ClockErrorBoundGeneric, ClockErrorBoundLayoutVersion, ClockErrorBoundV2,
        ClockErrorBoundV3, ShmError,
    },
    syserror,
};

/// A guard tracking an open file descriptor.
///
/// Creating the `FdGuard` opens the file with read-only permission.
/// The file descriptor is closed when the guard is dropped.
struct FdGuard(i32);

impl FdGuard {
    /// Create a new `FdGuard`.
    ///
    /// Open a file at `path` and store the open file descriptor
    fn new(path: &CStr) -> Result<Self, ShmError> {
        // SAFETY: `path` is a valid C string.
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY) };
        if fd < 0 {
            return syserror!(format!("Faild to open file at {:?}", path));
        }

        Ok(FdGuard(fd))
    }
}

impl Drop for FdGuard {
    /// Drop the `FdGuard` and close the file descriptor it holds.
    fn drop(&mut self) {
        // SAFETY: Unsafe because this is a call into a C API, but this particular
        // call is always safe.
        unsafe {
            let ret = libc::close(self.0);
            assert!(ret == 0 || errno() == Errno(libc::EINTR));
        }
    }
}

/// A guard tracking an memory mapped file.
///
/// Creating the `MmapGuard` maps an open file descriptor.
/// The file is unmap'ed when the guard is dropped.
#[derive(Debug)]
struct MmapGuard {
    /// A pointer to the head of the segment
    segment: *mut c_void,

    /// The size of the segment mapped into memory
    segsize: usize,
}

impl MmapGuard {
    /// Create a new `MmapGuard`.
    ///
    /// Map the open file descriptor held in the `FdGuard`.
    fn new(fdguard: &FdGuard) -> Result<Self, ShmError> {
        // Read the header so we know how much to map in memory.
        let header = ShmHeader::read(fdguard.0)?;

        // This consumes the segsize, but we only needed the header for validation and extracting
        // the segment size. So the move is fine here.
        let segsize = header.segsize.into_inner() as usize;

        // SAFETY: We're calling into a C function, but this particular call is always safe.
        let segment: *mut c_void = unsafe {
            libc::mmap(
                ptr::null_mut(),
                segsize,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fdguard.0,
                0,
            )
        };

        if segment == libc::MAP_FAILED {
            return syserror!(String::from("Failed to mmap the SHM segment"));
        }

        Ok(MmapGuard { segment, segsize })
    }
}

impl Drop for MmapGuard {
    /// Drop the `MmapGuard` and unmap the file it tracks.
    fn drop(&mut self) {
        // SAFETY: `segment` was previously returned from `mmap`, and therefore
        // when this destructor runs there are no more live references into
        // it.
        unsafe {
            let ret = libc::munmap(self.segment, self.segsize);
            assert_eq!(0, ret);
        }
    }
}

/// Reader for ClockBound daemon shared memory segment.
///
/// The Clockbound daemon shared memory segment consists of a `ShmHeader` followed by a
/// `ClockBoundError` struct. The segment is updated by a single producer (the clockbound daemon),
/// but may be read by many clients.  The shared memory segment does not implement a semaphore or
/// equivalent to synchronize the single-producer / many-consumers processes. Instead, the
/// mechanism is lock-free and relies on a `generation` number to ensure consistent reads (over
/// retries).
///
/// The writer increments the generation field from even to odd before each update. It also
/// increment it again, from odd to even, after finishing the update. Readers must check the
/// `generation` field before and after each read, and verify that they obtain the same, even,
/// value. Otherwise, the read was dirty and must be retried.
#[derive(Debug)]
pub struct ShmReader {
    // Drop guard to unmap the shared memory segment
    _guard: MmapGuard,

    // A raw pointer into the shared memory segment, pointing to the version member of the ShmHeader
    // section. The version number defines the shared memory segment content and layout. This is a
    // bit less flexible than a series of TLV but simpler (and not mutually exclusive).
    version: *const atomic::AtomicU16,

    // A raw pointer into the shared memory segment, pointing to the generation member of the
    // ShmHeader section. The generation number is used to read consistent snapshots of the shared
    // memory segment (that is outside of an update event by the writer). This is expected to roll
    // over as a function of the rate of update from the writer (eg. every ~9 hours if updating
    // once a second). A generation number equals to 0 signals the shared memory segment has not
    // been initialized.
    generation: *const atomic::AtomicU16,

    // A raw pointer into the shared memory segment, pointing to the ClockErrorBound section. Note
    // that the structured reference by this pointer may not be consistent, and reading it requires
    // to assert the generation value.
    ceb_shm: *const ClockErrorBound,

    // The last consistent snapshot of ClockErrorBound taken by this reader. It is what a read
    // falls back to while the writer is in the middle of an update, or has restarted and not
    // published data yet, instead of waiting for the writer to complete. It is refreshed only when
    // the writer generation has moved on since the last refresh, so the mutex is uncontended on
    // the read path.
    last_snapshot: Mutex<ClockErrorBound>,

    // The value of the writer generation when `last_snapshot` was refreshed, mirrored outside the
    // mutex so that reads can tell whether a refresh is due without taking it. Zero until the
    // first consistent read, a value no initialized segment ever reports.
    last_snapshot_gen: AtomicU16,
}

// SAFETY: The raw pointers point into a private, read-only (`PROT_READ`), `MAP_SHARED` mapping
// owned by the reader, which stays mapped until the reader is dropped. All accesses through them
// are atomic loads or volatile reads validated against the writer generation, none of which
// require exclusive access. The reader's own mutable state is behind a `Mutex` and an atomic.
// Nothing ties the mapping to the thread that created it, so it can be used and unmapped from any
// thread.
unsafe impl Send for ShmReader {}
unsafe impl Sync for ShmReader {}

impl ShmReader {
    /// Open a ClockBound shared memory segment for reading.
    ///
    /// On error, returns an appropriate `Errno`. If the content of the segment
    /// is uninitialized, unparseable, or otherwise malformed, EPROTO will be
    /// returned.
    #[expect(clippy::missing_errors_doc, reason = "todo")]
    pub fn new(path: &CStr) -> Result<ShmReader, ShmError> {
        // Map the segment, with explicit pointers to fields
        let (mmap_guard, version, generation, ceb_shm) = ShmReader::map_segment(path)?;

        // Atomically read the current version in the shared memory segment
        // SAFETY: `self.version` has been validated when creating the reader
        let shm_version = unsafe { &*version };
        let shm_version = shm_version.load(atomic::Ordering::Acquire);

        // FIXME: the ShmHeader should be versioned behind an enum instead.
        let min_version = shm_version >> 8;
        let max_version = shm_version & 0x00ff;
        if CLOCKBOUND_SHM_LATEST_VERSION < min_version
            || CLOCKBOUND_SHM_LATEST_VERSION > max_version
        {
            let msg = format!(
                "Clockbound shared memory segment supports versions {min_version} to {max_version} which does not include this reader version {CLOCKBOUND_SHM_LATEST_VERSION}",
            );
            return Err(ShmError::SegmentVersionNotSupported(msg));
        }
        let shm_version = ClockErrorBoundLayoutVersion::try_from(CLOCKBOUND_SHM_LATEST_VERSION)?;

        Ok(ShmReader {
            _guard: mmap_guard,
            version,
            generation,
            ceb_shm,
            last_snapshot: Mutex::new(ClockErrorBoundGeneric::builder().build(shm_version)),
            last_snapshot_gen: AtomicU16::new(0),
        })
    }

    /// Open a ClockBound shared memory segment for reading.
    ///
    /// On error, returns an appropriate `Errno`. If the content of the segment
    /// is uninitialized, unparseable, or otherwise malformed, EPROTO will be
    /// returned.
    #[expect(clippy::missing_errors_doc, reason = "todo")]
    pub fn new_with_max_version_unchecked(path: &CStr) -> Result<(ShmReader, u16), ShmError> {
        // Map the segment, with explicit pointers to fields
        let (mmap_guard, version, generation, ceb_shm) = ShmReader::map_segment(path)?;

        // Atomically read the current version from the shared memory segment
        // SAFETY: `self.version` has been validated when creating the reader
        let shm_version = unsafe { &*version };
        let shm_version = shm_version.load(atomic::Ordering::Acquire);
        let max_version = shm_version & 0x00ff;

        let current_version =
            ClockErrorBoundLayoutVersion::try_from(CLOCKBOUND_SHM_LATEST_VERSION)?;

        Ok((
            ShmReader {
                _guard: mmap_guard,
                version,
                generation,
                ceb_shm,
                last_snapshot: Mutex::new(ClockErrorBoundGeneric::builder().build(current_version)),
                last_snapshot_gen: AtomicU16::new(0),
            },
            max_version,
        ))
    }

    /// Open and map the ClockBound shared memory segment.
    ///
    /// Make sure the file can be open, and that the raw pointers into memory are set.
    ///
    /// # Errors
    ///
    /// Returns `ShmError` if the path is not found, the file cannot be open or sanity checks fail.
    fn map_segment(
        path: &CStr,
    ) -> Result<
        (
            MmapGuard,
            *const AtomicU16,
            *const AtomicU16,
            *const ClockErrorBound,
        ),
        ShmError,
    > {
        let fdguard = FdGuard::new(path)?;
        let mmap_guard = MmapGuard::new(&fdguard)?;

        // Create a cursor to pick the addresses of the various elements of interest in the shared
        // memory segment.
        let mut cursor: *const u8 = mmap_guard.segment.cast();

        // Pick fields from the ShmHeader
        // SAFETY: `cursor` is aligned to the start of the memory segment and the MmapGuard has
        // validated the memory segment is large enough to contain the header.
        let version = unsafe { ptr::addr_of!((*cursor.cast::<ShmHeader>()).version) };
        let generation = unsafe { ptr::addr_of!((*cursor.cast::<ShmHeader>()).generation) };

        // Atomically read the current version in the shared memory segment
        // SAFETY: `self.version` has been validated when creating the reader
        let shm_version = unsafe { &*version };
        let shm_version = shm_version.load(atomic::Ordering::Acquire);

        // FIXME: temporary workaround waiting for https://github.com/aws/private-clock-bound-staging/pull/150
        // to be pulled in
        let shm_version = ClockErrorBoundLayoutVersion::try_from(shm_version & 0x00ff)?;

        // Move to the end of the header and map the ClockErrorBound data, but only if the segment
        // size allows it and matches our expectation.
        let layout_size = match shm_version {
            ClockErrorBoundLayoutVersion::V2 => size_of::<ClockErrorBoundV2>(),
            ClockErrorBoundLayoutVersion::V3 => size_of::<ClockErrorBoundV3>(),
        };

        if mmap_guard.segsize < size_of::<ShmHeader>() + layout_size {
            let msg = format!(
                "Clockbound segment size is smaller than expected [{} < {}].",
                mmap_guard.segsize,
                size_of::<ShmHeader>() + layout_size
            );
            return Err(ShmError::SegmentMalformed(msg));
        }

        // SAFETY: segment size has been checked to ensure `cursor` move leads to a valid cast
        cursor = unsafe { cursor.add(size_of::<ShmHeader>()) };
        let ceb_shm = ptr::addr_of!(*cursor.cast::<ClockErrorBound>());

        Ok((mmap_guard, version, generation, ceb_shm))
    }

    /// Return a consistent snapshot of the shared memory segment.
    ///
    /// Taking a snapshot consists in reading the memory segment while confirming the generation
    /// number in the header has not changed (which would indicate an update from the writer
    /// occurred while reading). If an update is detected, the read is retried.
    ///
    /// The snapshot is returned by value: the `ClockErrorBound` data is small, and copying it out
    /// of the segment is what lets a single `ShmReader` be shared across threads. Every call reads
    /// the shared memory segment, except while the writer is in the middle of an update or has
    /// restarted and not published data yet: the last consistent snapshot taken by this reader is
    /// then returned instead of waiting, yielding or retrying excessively.
    #[expect(clippy::missing_errors_doc, reason = "todo")]
    pub fn snapshot(&self) -> Result<ClockErrorBound, ShmError> {
        // Atomically read the current version in the shared memory segment
        // SAFETY: `self.version` has been validated when creating the reader
        let version = unsafe { &*self.version };
        let version = version.load(atomic::Ordering::Acquire);

        // The version number is checked when the reader is created to not be 0. If we now see a
        // version equal to 0, the writer has restarted, wiped the segment clean, but has not
        // defined the layout yet. Choose to return the last snapshot. If the writer died in the
        // middle of restarting, the snapshot will eventually be stale. Enough information is
        // returned to the caller to take appropriate action (e.g. assert clock status).
        if version == 0 {
            return Ok(self.last_snapshot());
        }

        // It is possible the daemon has been upgraded and restarted. It is meant to be backward
        // compatible when writing to the default path to the shared memory segment. That is, the
        // current version written by the daemon may have incremented, but the minimum version
        // supported must ours or lower. In the other direction, if the clockbound daemon was
        // downgraded, also have to report errors, since expected features may be missing.
        let min_version = version >> 8;
        let max_version = version & 0x00ff;
        if CLOCKBOUND_SHM_LATEST_VERSION < min_version
            || CLOCKBOUND_SHM_LATEST_VERSION < max_version
        {
            let msg = format!(
                "Clockbound shared memory segment supports versions {min_version} to {max_version} which does not include this reader version {CLOCKBOUND_SHM_LATEST_VERSION}",
            );
            return Err(ShmError::SegmentVersionNotSupported(msg));
        }

        // Atomically read the current generation in the shared memory segment
        // SAFETY: `self.generation` has been validated when creating the reader
        let generation = unsafe { &*self.generation };
        let mut first_gen = generation.load(atomic::Ordering::Acquire);

        // The generation number is checked when the reader is created to not be 0. If we now see a
        // generation equals to 0, the writer has restarted, wiped the segment clean, but has not
        // initialized it with valid data yet. Choose to return the last snapshot. If the writer
        // died in the middle of restarting, the snapshot will eventually be stale. Enough
        // information is returned to the caller to take appropriate action (e.g. assert clock
        // status).
        if first_gen == 0 {
            return Ok(self.last_snapshot());
        }

        // If the generation is an odd number, the shared memory segment is in the process of being
        // updated by the writer. Instead of waiting, yielding or busy looping, simply return the
        // last snapshot taken. It is fine for the reader to return a bound on clock error based on
        // the previously updated shared memory segment. The bound on clock error returned would be
        // larger than it could have been, but still correct. If the writer died in the middle of
        // an update, the snapshot will eventually be stale. The caller is returned enough
        // information to act accordingly.
        if first_gen & 0x0001 == 1 {
            return Ok(self.last_snapshot());
        }

        // The generation number is an even number. Loop until we obtain a consistent read of the
        // clock error bound data. This relies on reading the generation value twice, making sure
        // they are identical and an even number.
        //
        // The writer could die in the middle of the update. This could lead to not making any
        // progress hence capping the number of retries.
        let mut retries = 1_000_000;
        while retries > 0 {
            // Read the ClockErrorBound data from the shared memory
            // SAFETY: `ceb_at` has been checked to be valid while creating the ShmReader
            let snapshot = unsafe { self.ceb_shm.read_volatile() };

            // Confirm no update occurred during the read
            let second_gen = generation.load(atomic::Ordering::Acquire);
            if first_gen == second_gen {
                // Keep the fallback snapshot current, but only when the writer has moved on since
                // it was last refreshed. This keeps the mutex off the read path, which only sees
                // the generation change at the rate of writer updates.
                //
                // Note that the generation number DOES roll over, but never take a value of 0 once
                // the segment is initialized. It is still possible that the generation number
                // matches although the counter has rolled over, in which case the fallback is not
                // refreshed this time around. Assuming one update per sec, this leaves a collision
                // probability of 1 / 2^16, and a rollover once every 18 hours. Although the risk is
                // very small it exists, but the `void_after` member on the ClockErrorBound struct
                // can be used to provide an additional layer of protection.
                if self.last_snapshot_gen.load(atomic::Ordering::Acquire) != first_gen {
                    self.remember(first_gen, snapshot);
                }
                return Ok(snapshot);
            }
            // Only track complete updates indicated by an even generation number.
            if second_gen & 0x0001 == 0 {
                first_gen = second_gen;
            }
            retries -= 1;
        }

        // Attempts to read the snapshot have failed.
        Err(ShmError::SegmentNotInitialized(String::from(
            "Failed to read the SHM segment after all attempts",
        )))
    }

    /// Return the last consistent snapshot taken by this reader.
    fn last_snapshot(&self) -> ClockErrorBound {
        *self
            .last_snapshot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Record the snapshot taken at `generation` as the fallback for reads made while the writer
    /// is updating the segment.
    ///
    /// Concurrent readers may race here with snapshots from different generations, and whichever
    /// stores last wins. This is harmless: any consistent snapshot is a valid, if slightly more
    /// pessimistic, bound on clock error, and the next read to see a newer generation refreshes
    /// the fallback again.
    fn remember(&self, generation: u16, snapshot: ClockErrorBound) {
        let mut last_snapshot = self
            .last_snapshot
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *last_snapshot = snapshot;
        self.last_snapshot_gen
            .store(generation, atomic::Ordering::Release);
    }
}

#[cfg(test)]
mod t_reader {
    use super::*;
    use crate::shm::ClockStatus;
    use byteorder::{NativeEndian, WriteBytesExt};
    use nix::sys::time::TimeSpec;
    use std::ffi::CString;
    use std::fs::OpenOptions;
    use std::io::Seek;
    use std::io::Write;
    /// We make use of tempfile::NamedTempFile to ensure that
    /// local files that are created during a test get removed
    /// afterwards.
    use tempfile::NamedTempFile;

    macro_rules! write_memory_segment {
        ($file:ident,
         $magic_0:literal,
         $magic_1:literal,
         $segsize:literal,
         $version:literal,
         $generation:literal,
         ($as_of_sec:literal, $as_of_nsec:literal),
         ($void_after_sec:literal, $void_after_nsec:literal),
         $bound_nsec:literal,
         $max_drift: literal) => {
            // Build the bound on clock error data
            let ceb = ClockErrorBoundGeneric::builder()
                .as_of(TimeSpec::new($as_of_sec, $as_of_nsec))
                .void_after(TimeSpec::new($void_after_sec, $void_after_nsec))
                .bound_nsec($bound_nsec)
                .disruption_marker(0)
                .max_drift_ppb($max_drift)
                .clock_status(ClockStatus::Synchronized)
                .clock_disruption_support_enabled(true)
                .build(ClockErrorBoundLayoutVersion::V2);

            // Convert the ceb struct into a slice so we can write it all out, fairly magic.
            // Definitely needs the #[repr(C)] layout.
            let slice = unsafe {
                ::core::slice::from_raw_parts(
                    (&ceb as *const ClockErrorBound) as *const u8,
                    ::core::mem::size_of::<ClockErrorBound>(),
                )
            };

            $file
                .write_u32::<NativeEndian>($magic_0)
                .expect("Write failed magic_0");
            $file
                .write_u32::<NativeEndian>($magic_1)
                .expect("Write failed magic_1");
            $file
                .write_u32::<NativeEndian>($segsize)
                .expect("Write failed segsize");
            $file
                .write_u16::<NativeEndian>($version)
                .expect("Write failed version");
            $file
                .write_u16::<NativeEndian>($generation)
                .expect("Write failed generation");
            $file
                .write_all(slice)
                .expect("Write failed ClockErrorBound");
            $file.sync_all().expect("Sync to disk failed");
        };
    }

    /// Assert that the reader can map a file.
    #[test]
    fn test_reader_new_shm_v2() {
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        let mut clockbound_shm_file = OpenOptions::new()
            .write(true)
            .open(clockbound_shm_path)
            .expect("open clockbound file failed");
        write_memory_segment!(
            clockbound_shm_file,
            0x414D5A4E,
            0x43420200,
            400,
            0x0002,
            10,
            (0, 0),
            (0, 0),
            123,
            0
        );
        let path = CString::new(clockbound_shm_path).expect("CString failed");

        // This should fail with a version mismatch error
        let res = ShmReader::new(&path);
        assert!(res.is_err());
    }

    /// Assert that the reader can map a file.
    #[test]
    fn test_reader_new_shm_v3() {
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        let mut clockbound_shm_file = OpenOptions::new()
            .write(true)
            .open(clockbound_shm_path)
            .expect("open clockbound file failed");
        write_memory_segment!(
            clockbound_shm_file,
            0x414D5A4E,
            0x43420200,
            400,
            0x0303,
            10,
            (0, 0),
            (0, 0),
            123,
            0
        );

        let path = CString::new(clockbound_shm_path).expect("CString failed");
        let reader = ShmReader::new(&path).expect("Failed to create ShmReader");

        let version = unsafe { &*reader.version };
        let generation = unsafe { &*reader.generation };
        let ceb = unsafe { *reader.ceb_shm };

        assert_eq!(version.load(atomic::Ordering::Relaxed), 0x0303);
        assert_eq!(generation.load(atomic::Ordering::Relaxed), 10);
        assert_eq!(ceb.bound_nsec(), 123);
    }

    /// Assert that creating a reader when the
    /// shared memory segment has an unsupported version causes an Err result.
    #[test]
    fn test_reader_new_of_unsupported_shm_version() {
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        let mut clockbound_shm_file = OpenOptions::new()
            .write(true)
            .open(clockbound_shm_path)
            .expect("open clockbound file failed");
        write_memory_segment!(
            clockbound_shm_file,
            0x414D5A4E,
            0x43420200,
            400,
            9999,
            10,
            (0, 0),
            (0, 0),
            123,
            0
        );

        let path = CString::new(clockbound_shm_path).expect("CString failed");
        let result = ShmReader::new(&path);

        // Assert that creating a reader on an unsupported shared memory segment version
        // returns Err(ShmError::SegmentVersionNotSupported).
        assert!(matches!(
            result.unwrap_err(),
            ShmError::SegmentVersionNotSupported(_)
        ));
    }

    /// Assert that creating a reader and taking a snapshot when the
    /// shared memory segment has an unsupported version causes an Err result.
    #[test]
    fn test_reader_snapshot_of_unsupported_shm_version() {
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        let mut clockbound_shm_file = OpenOptions::new()
            .write(true)
            .open(clockbound_shm_path)
            .expect("open clockbound file failed");
        // Initially, write the current supported version.
        write_memory_segment!(
            clockbound_shm_file,
            0x414D5A4E,
            0x43420200,
            400,
            0x0303,
            10,
            (0, 0),
            (0, 0),
            123,
            0
        );

        let path = CString::new(clockbound_shm_path).expect("CString failed");
        let reader = ShmReader::new(&path).expect("Failed to create ShmReader");
        let version = unsafe { &*reader.version };
        assert_eq!(version.load(atomic::Ordering::Relaxed), 0x0303);

        // Assert that snapshot works without an error with this supported version.
        let result = reader.snapshot();
        assert!(result.is_ok());

        // Update the shared memory segment so that the version number is an
        // unsupported number (e.g. 9999).
        let _ = clockbound_shm_file.rewind();
        write_memory_segment!(
            clockbound_shm_file,
            0x414D5A4E,
            0x43420200,
            400,
            9999,
            10,
            (0, 0),
            (0, 0),
            123,
            0
        );

        let version = unsafe { &*reader.version };
        assert_eq!(version.load(atomic::Ordering::Relaxed), 9999);

        // Assert that taking a snapshot of an unsupported shared memory segment version
        // returns Err(ShmError::SegmentVersionNotSupported).
        let result = reader.snapshot();
        assert!(
            matches!(result.unwrap_err(), ShmError::SegmentVersionNotSupported(msg) if msg.starts_with("Clockbound shared memory segment supports versions"))
        );
    }

    /// Assert that a snapshot falls back to the last consistent one while the writer is in the
    /// middle of an update or has restarted, and picks up new data once an update is complete.
    #[test]
    fn test_reader_snapshot_falls_back_while_writer_updates() {
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        let mut clockbound_shm_file = OpenOptions::new()
            .write(true)
            .open(clockbound_shm_path)
            .expect("open clockbound file failed");
        write_memory_segment!(
            clockbound_shm_file,
            0x414D5A4E,
            0x43420200,
            400,
            0x0303,
            10,
            (0, 0),
            (0, 0),
            123,
            0
        );

        let path = CString::new(clockbound_shm_path).expect("CString failed");
        let reader = ShmReader::new(&path).expect("Failed to create ShmReader");
        let snapshot = reader.snapshot().expect("Failed to take a snapshot");
        assert_eq!(snapshot.bound_nsec(), 123);

        // An odd generation marks an update in progress: the previous snapshot is returned.
        let _ = clockbound_shm_file.rewind();
        write_memory_segment!(
            clockbound_shm_file,
            0x414D5A4E,
            0x43420200,
            400,
            0x0303,
            11,
            (0, 0),
            (0, 0),
            456,
            0
        );
        let snapshot = reader.snapshot().expect("Failed to take a snapshot");
        assert_eq!(snapshot.bound_nsec(), 123);

        // The update completed: the new data is returned.
        let _ = clockbound_shm_file.rewind();
        write_memory_segment!(
            clockbound_shm_file,
            0x414D5A4E,
            0x43420200,
            400,
            0x0303,
            12,
            (0, 0),
            (0, 0),
            456,
            0
        );
        let snapshot = reader.snapshot().expect("Failed to take a snapshot");
        assert_eq!(snapshot.bound_nsec(), 456);

        // A generation of zero marks a writer that restarted and has not published data yet.
        let _ = clockbound_shm_file.rewind();
        write_memory_segment!(
            clockbound_shm_file,
            0x414D5A4E,
            0x43420200,
            400,
            0x0303,
            0,
            (0, 0),
            (0, 0),
            789,
            0
        );
        let snapshot = reader.snapshot().expect("Failed to take a snapshot");
        assert_eq!(snapshot.bound_nsec(), 456);
    }

    /// Assert that one reader can be shared across threads while the writer keeps updating the
    /// segment: every snapshot is one of the values written, never a torn or failed read.
    #[test]
    fn test_reader_shared_across_threads_with_concurrent_writer() {
        use crate::shm::{ShmWrite, ShmWriter};

        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ShmReader>();

        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        let mut writer = ShmWriter::new(
            std::path::Path::new(clockbound_shm_path),
            ClockErrorBoundLayoutVersion::V3,
            ClockErrorBoundLayoutVersion::V3,
        )
        .expect("Failed to create a writer");
        let cebs = [123, 456].map(|bound_nsec| {
            ClockErrorBoundGeneric::builder()
                .bound_nsec(bound_nsec)
                .clock_status(ClockStatus::Synchronized)
                .build(ClockErrorBoundLayoutVersion::V3)
        });
        writer.write(&cebs[0]);

        let path = CString::new(clockbound_shm_path).expect("CString failed");
        let reader = ShmReader::new(&path).expect("Failed to create ShmReader");
        // Seed the fallback snapshot so a read that lands in the middle of an update below is
        // still one of the values written.
        assert_eq!(
            reader.snapshot().expect("Failed to take a snapshot"),
            cebs[0]
        );

        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    for _ in 0..10_000 {
                        let snapshot = reader.snapshot().expect("Failed to take a snapshot");
                        assert!(cebs.contains(&snapshot), "torn read: {snapshot:?}");
                    }
                });
            }
            for i in 0..10_000 {
                writer.write(&cebs[i % 2]);
            }
        });
    }
}
