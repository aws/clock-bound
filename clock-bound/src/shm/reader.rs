#![expect(clippy::cast_ptr_alignment, reason = "TODO COME BACK TO THIS")]

use errno::{Errno, errno};
use std::ffi::{CStr, c_void};
use std::mem::size_of;
use std::ptr;
use std::sync::atomic::{self, AtomicU16};

use crate::shm::shm_header::{CLOCKBOUND_SHM_LATEST_VERSION, ShmHeader};
use crate::shm::{
    ClockErrorBound, ClockErrorBoundGeneric, ClockErrorBoundLayoutVersion, ClockErrorBoundV2,
    ClockErrorBoundV3, ShmError, syserror,
};

/// Typed pointer to the inner `#[repr(C)]` payload stored in the shared
/// memory segment. The segment holds the raw payload struct (never the
/// `ClockErrorBound` enum, which has no stable layout), so the reader must
/// decode through the variant matching the segment's layout version.
#[derive(Debug)]
enum LayoutPtr {
    V2(*const ClockErrorBoundV2),
    V3(*const ClockErrorBoundV3),
}

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
            return syserror!(format!("Failed to open file at {:?}", path));
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
/// `ClockErrorBound` struct. The segment is updated by a single producer (the clockbound daemon),
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
    // Explicitly make the ShmReader be !Send and !Sync, since it is not thread safe. A bit ugly to
    // use a phantom raw pointer, but effective and free at runtime.
    _marker: std::marker::PhantomData<*const ()>,

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

    // A typed pointer into the shared memory segment, pointing to the inner `#[repr(C)]` payload
    // selected by the segment's layout version (never the layout-unstable `ClockErrorBound`
    // enum). Note that the structure referenced by this pointer may not be consistent, and
    // reading it requires to assert the generation value.
    ceb_shm: LayoutPtr,

    // The last snapshot of ClockErrorBound taken. This acts as a cache to avoid waiting for the
    // writer to complete an update and allow to share a reference to this memory location
    // (avoiding some memory copy). Keeping a state here and sharing it with the caller makes the
    // ShmReader not thread safe.
    snapshot_ceb: ClockErrorBound,

    // The value of the writer generation when the ceb snapshot was taken.
    snapshot_gen: u16,
}

// SAFETY: This struct holds exclusive access to the raw memory mapped pointers and
// can be sent to other threads. No limitations from mmap
unsafe impl Send for ShmReader {}

// SAFETY: It is Sync because this struct does not mutate the underlying pointers.
// All mutations require `&mut self` (which only affect the non-pointer fields anyways)
unsafe impl Sync for ShmReader {}

impl ShmReader {
    /// Open a ClockBound shared memory segment for reading.
    ///
    /// On error, returns an appropriate `Errno`. If the content of the segment
    /// is uninitialized, unparseable, or otherwise malformed, EPROTO will be
    /// returned.
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
            _marker: std::marker::PhantomData,
            _guard: mmap_guard,
            version,
            generation,
            ceb_shm,
            snapshot_ceb: ClockErrorBoundGeneric::builder().build(shm_version),
            snapshot_gen: 0,
        })
    }

    /// Open a ClockBound shared memory segment for reading.
    ///
    /// On error, returns an appropriate `Errno`. If the content of the segment
    /// is uninitialized, unparseable, or otherwise malformed, EPROTO will be
    /// returned.
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
                _marker: std::marker::PhantomData,
                _guard: mmap_guard,
                version,
                generation,
                ceb_shm,
                snapshot_ceb: ClockErrorBoundGeneric::builder().build(current_version),
                snapshot_gen: 0,
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
    ) -> Result<(MmapGuard, *const AtomicU16, *const AtomicU16, LayoutPtr), ShmError> {
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

        // SAFETY: the segment-size check above guarantees `layout_size` bytes are mapped past the
        // header, so advancing `cursor` by the header size leaves it in-bounds. The cast target is
        // the concrete `#[repr(C)]` payload type selected by the validated layout version (never
        // the layout-unstable `ClockErrorBound` enum); the byte offset is unchanged.
        cursor = unsafe { cursor.add(size_of::<ShmHeader>()) };
        let ceb_shm = match shm_version {
            ClockErrorBoundLayoutVersion::V2 => LayoutPtr::V2(cursor.cast::<ClockErrorBoundV2>()),
            ClockErrorBoundLayoutVersion::V3 => LayoutPtr::V3(cursor.cast::<ClockErrorBoundV3>()),
        };

        Ok((mmap_guard, version, generation, ceb_shm))
    }

    /// Return a consistent snapshot of the shared memory segment.
    ///
    /// Taking a snapshot consists in reading the memory segment while confirming the generation
    /// number in the header has not changed (which would indicate an update from the writer
    /// occurred while reading). If an update is detected, the read is retried.
    ///
    /// This function returns a reference to the `ClockErrorBound` snapshot stored by the reader, and
    /// not an owned value. This make the `ShmReader` NOT thread-safe: the data pointed to could be
    /// updated without one of the thread knowing, leading to a incorrect clock error bond. The
    /// advantage are in terms of performance: less data copied, but also no locking, yielding or
    /// excessive retries.
    pub fn snapshot(&mut self) -> Result<&ClockErrorBound, ShmError> {
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
            return Ok(&self.snapshot_ceb);
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
            return Ok(&self.snapshot_ceb);
        }

        // Quick optimization, if the generation number matches the last one recorded, the shared
        // memory segment has not been updated since last read. No need to read more of the memory
        // segment, instead return the reference to the snapshot. This is useful in cases where the
        // rate of clockbound read is much higher than the rate of write to the shared memory
        // segment.
        //
        // Note that the generation number DOES roll over, but never take a value of 0 once the
        // segment is initialized. It is still possible that the generation number matches although
        // the counter has rolled over. Assuming one update per sec, this leaves a collision
        // probability of 1 / 2^16, and a rollover once every 18 hours. Although the risk is very
        // small it exists, but the `void_after` member on the ClockErrorBound struct can be used
        // to provide an additional layer of protection.
        if first_gen == self.snapshot_gen {
            return Ok(&self.snapshot_ceb);
        }

        // If the generation is an odd number, the shared memory segment is in the process of being
        // updated by the writer. Instead of waiting, yielding or busy looping, simply return the
        // last snapshot taken. It is fine for the reader to return a bound on clock error based on
        // the previously updated shared memory segment. The bound on clock error returned would be
        // larger than it could have been, but still correct. If the writer died in the middle of
        // an update, the snapshot will eventually be stale. The caller is returned enough
        // information to act accordingly.
        if first_gen & 0x0001 == 1 {
            return Ok(&self.snapshot_ceb);
        }

        // The generation number is an even number, and has changed since the last snapshot. Loop
        // until we obtain a consistent read of the clock error bound data. This relies on reading
        // the generation value twice, making sure they are identical and an even number.
        //
        // The writer could die in the middle of the update. This could lead to not making any
        // progress hence capping the number of retries.
        let mut retries = 1_000_000;
        while retries > 0 {
            // Read the ClockErrorBound data from the shared memory
            // SAFETY: `ceb_shm` was built in `map_segment` from the validated layout version,
            // points `layout_size` bytes inside the mapped segment, and is aligned for the
            // concrete `#[repr(C)]` payload. `read_volatile` copies the payload out; the
            // generation check below confirms the read was not torn.
            let snapshot = unsafe {
                match self.ceb_shm {
                    LayoutPtr::V2(p) => ClockErrorBound::V2(p.read_volatile()),
                    LayoutPtr::V3(p) => ClockErrorBound::V3(p.read_volatile()),
                }
            };

            // Confirm no update occurred during the read
            let second_gen = generation.load(atomic::Ordering::Acquire);
            if first_gen == second_gen {
                self.snapshot_gen = first_gen;
                self.snapshot_ceb = snapshot;
                return Ok(&self.snapshot_ceb);
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
}

#[cfg(test)]
mod t_reader {
    use super::*;
    use crate::shm::ClockStatus;
    use crate::shm::shm_header::SHM_MAGIC;
    use byteorder::{NativeEndian, WriteBytesExt};
    use bytes::BufMut;
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
    fn reader_new_shm_v2() {
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
    fn reader_new_shm_v3() {
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();

        // The segment header advertises version 0x0303 (V3), so the data region must be a V3
        // payload. Lay the V3 layout down field by field (the same approach as
        // `reader_decodes_v3_protocol_layout`) with a `bound_nsec` of 123 so the reader decodes a
        // self-consistent V3 segment.
        let mut body: Vec<u8> = Vec::new();
        body.put_u16_ne(0x0303); // Version word: min=3, max=3.
        body.put_u16_ne(10); // Generation.
        // --- Data region: Protocol Version 3 layout ---
        body.put_u64_ne(0); // As-Of TSC
        body.put_i64_ne(0); // As-Of Timestamp tv_sec
        body.put_i64_ne(0); // As-Of Timestamp tv_nsec
        body.put_i64_ne(0); // Void-After tv_sec
        body.put_i64_ne(0); // Void-After tv_nsec
        body.put_u64_ne(0); // Period
        body.put_u64_ne(0); // Period Error
        body.put_i64_ne(123); // Bound
        body.put_u64_ne(0); // Disruption Marker
        body.put_u32_ne(0); // Max Drift
        body.put_i32_ne(ClockStatus::Synchronized as i32); // Clock Status
        body.put_u8(1); // Clock Disruption Support
        body.put_u8(0); // Period Shift
        body.put_u8(0); // Period Error Shift

        // size_of::<ShmHeader>() (16) + size_of::<ClockErrorBoundV3>() (88) == 104.
        write_segment_bytes(clockbound_shm_path, 104, &body);

        let path = CString::new(clockbound_shm_path).expect("CString failed");
        let reader = ShmReader::new(&path).expect("Failed to create ShmReader");

        let version = unsafe { &*reader.version };
        let generation = unsafe { &*reader.generation };
        let ceb = if let LayoutPtr::V3(p) = reader.ceb_shm {
            // SAFETY: the segment was written as a V3 layout, so the V3 payload pointer is valid.
            unsafe { *p }
        } else {
            panic!("expected a V3 layout pointer");
        };

        assert_eq!(version.load(atomic::Ordering::Relaxed), 0x0303);
        assert_eq!(generation.load(atomic::Ordering::Relaxed), 10);
        assert_eq!(ceb.bound_nsec, 123);
    }

    /// Assert that creating a reader when the
    /// shared memory segment has an unsupported version causes an Err result.
    #[test]
    fn reader_new_of_unsupported_shm_version() {
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
    fn reader_snapshot_of_unsupported_shm_version() {
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
        let mut reader = ShmReader::new(&path).expect("Failed to create ShmReader");
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

    /// Lay out a raw segment by hand, byte by byte, and write it to `path`.
    ///
    /// The bytes are produced in the exact field order documented in `docs/protocol.md`,
    /// using native-endian (`_ne`) puts to match the protocol's "native endian of the CPU" contract.
    ///
    /// We are testing the wire protocol: nothing here goes through the
    /// `ClockErrorBound` enum. If the reader decodes the data region at anything other than the
    /// documented offsets (for example because it expects an enum discriminant before the payload),
    /// the snapshot it returns will not match these hand-placed values and the test fails.
    fn write_segment_bytes(path: &str, segsize: usize, body: &[u8]) {
        let mut buf = Vec::with_capacity(segsize);
        // ShmHeader (repr(C, align(8))) first three fields: magic (two u32 words) and segsize.
        // These are the only header bytes this helper owns; `body` supplies everything from the
        // version word onward (version, generation, then the data region).
        buf.put_u32_ne(SHM_MAGIC[0]);
        buf.put_u32_ne(SHM_MAGIC[1]);
        buf.put_u32_ne(u32::try_from(segsize).expect("segsize fits in u32"));
        buf.extend_from_slice(body);
        assert!(
            buf.len() <= segsize,
            "segment body ({}) exceeds segsize ({segsize})",
            buf.len()
        );
        buf.resize(segsize, 0); // zero-fill trailing padding up to segsize

        let mut file = OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open clockbound file failed");
        file.write_all(&buf).expect("write segment failed");
        file.sync_all().expect("sync_all failed");
    }

    #[test]
    fn reader_decodes_v3_protocol_layout() {
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();

        // Deterministic, bitwise-unique field values.
        let as_of_tsc: u64 = 0x0102_0304_0506_0708;
        let as_of = TimeSpec::new(0x1111_1111, 0x2222_2222);
        let void_after = TimeSpec::new(0x3333_3333, 0x4444_4444);
        let period_frac: u64 = 0x0A0B_0C0D_0E0F_1011;
        let period_err_frac: u64 = 0x1213_1415_1617_1819;
        let bound_nsec: i64 = 0x5555_5555_5555_5555;
        let disruption_marker: u64 = 0x6666_6666_6666_6666;
        let max_drift_ppb: u32 = 0x0777_7777; // < 1_000_000_000
        let clock_status = ClockStatus::Disrupted; // == 3
        let period_shift: u8 = 0x2A;
        let period_err_shift: u8 = 0x3B;

        let mut body: Vec<u8> = Vec::new();
        // Version word: V3 uses min=3, max=3 => 0x0303.
        body.put_u16_ne(0x0303);
        // Generation: even and non-zero.
        body.put_u16_ne(2);
        // --- Data region: Protocol Version 3 layout ---
        body.put_u64_ne(as_of_tsc); // As-Of TSC
        body.put_i64_ne(as_of.tv_sec()); // As-Of Timestamp tv_sec
        body.put_i64_ne(as_of.tv_nsec()); // As-Of Timestamp tv_nsec
        body.put_i64_ne(void_after.tv_sec()); // Void-After tv_sec
        body.put_i64_ne(void_after.tv_nsec()); // Void-After tv_nsec
        body.put_u64_ne(period_frac); // Period
        body.put_u64_ne(period_err_frac); // Period Error
        body.put_i64_ne(bound_nsec); // Bound
        body.put_u64_ne(disruption_marker); // Disruption Marker
        body.put_u32_ne(max_drift_ppb); // Max Drift
        body.put_i32_ne(clock_status as i32); // Clock Status
        body.put_u8(1); // Clock Disruption Support
        body.put_u8(period_shift); // Period Shift
        body.put_u8(period_err_shift); // Period Error Shift
        // Remaining bytes are padding, zero-filled by write_segment_bytes up to segsize.

        // size_of::<ShmHeader>() (16) + size_of::<ClockErrorBoundV3>() (88) == 104.
        write_segment_bytes(clockbound_shm_path, 104, &body);

        let path = CString::new(clockbound_shm_path).expect("CString failed");
        let mut reader = ShmReader::new(&path).expect("Failed to create ShmReader");
        let snapshot = *reader.snapshot().expect("snapshot failed");

        let ClockErrorBound::V3(ceb) = snapshot else {
            panic!("expected V3");
        };
        assert_eq!(ceb.as_of_counter(), as_of_tsc);
        assert_eq!(snapshot.as_of(), as_of);
        assert_eq!(snapshot.void_after(), void_after);
        assert_eq!(ceb.period_frac, period_frac);
        assert_eq!(ceb.period_err_frac, period_err_frac);
        assert_eq!(snapshot.bound_nsec(), bound_nsec);
        assert_eq!(snapshot.disruption_marker(), disruption_marker,);
        assert_eq!(snapshot.max_drift_ppb(), max_drift_ppb);
        assert_eq!(snapshot.clock_status(), clock_status);
        assert!(snapshot.clock_disruption_support_enabled(),);
        assert_eq!(ceb.period_shift, period_shift);
        assert_eq!(ceb.period_err_shift, period_err_shift);
    }
}
