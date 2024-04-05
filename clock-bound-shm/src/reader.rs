// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use errno::{errno, Errno};
use std::ffi::{c_void, CStr};
use std::mem::size_of;
use std::ptr;
use std::sync::atomic;

use crate::shm_header::ShmHeader;
use crate::{syserror, ClockErrorBound, ShmError};

/// A guard tracking an open file descriptor.
///
/// Creating the FdGuard opens the file with read-only permission.
/// The file descriptor is closed when the guard is dropped.
struct FdGuard(i32);

impl FdGuard {
    /// Create a new FdGuard.
    ///
    /// Open a file at `path` and store the open file descriptor
    fn new(path: &CStr) -> Result<Self, ShmError> {
        // SAFETY: `path` is a valid C string.
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY) };
        if fd < 0 {
            return syserror!(concat!("open"));
        }

        Ok(FdGuard(fd))
    }
}

impl Drop for FdGuard {
    /// Drop the FdGuard and close the file descriptor it holds.
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
/// Creating the MmapGuard maps an open file descriptor.
/// The file is unmap'ed when the guard is dropped.
struct MmapGuard {
    /// A pointer to the head of the segment
    segment: *mut c_void,

    /// The size of the segment mapped into memory
    segsize: usize,
}

impl MmapGuard {
    /// Create a new MmapGuard.
    ///
    /// Map the open file descriptor held in the FdGuard.
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
            return syserror!("mmap SHM segment");
        }

        Ok(MmapGuard { segment, segsize })
    }
}

impl Drop for MmapGuard {
    /// Drop the MmapGuard and unmap the file it tracks.
    fn drop(&mut self) {
        // SAFETY: `segment` was previously returned from `mmap`, and therefore
        // when this destructor runs there are no more live references into
        // it.
        unsafe {
            let ret = libc::munmap(self.segment, self.segsize);
            assert!(ret == 0);
        }
    }
}

/// Reader for ClockBound daemon shared memory segment.
///
/// The Clockbound daemon shared memory segment consists of a ShmHeader followed by a
/// ClockBoundError struct. The segment is updated by a single producer (the clockbound daemon),
/// but may be read by many clients.  The shared memory segment does not implement a semaphore or
/// equivalent to synchronize the single-producer / many-consumers processes. Instead, the
/// mechanism is lock-free and relies on a `generation` number to ensure consistent reads (over
/// retries).
///
/// The writer increments the generation field from even to odd before each update. It also
/// increment it again, from odd to even, after finishing the update. Readers must check the
/// `generation` field before and after each read, and verify that they obtain the same, even,
/// value. Otherwise, the read was dirty and must be retried.
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

    // A raw pointer into the shared memory segment, pointing to the ClockErrorBound section. Note
    // that the structured reference by this pointer may not be consistent, and reading it requires
    // to assert the generation value.
    ceb_shm: *const ClockErrorBound,

    // The last snapshot of ClockErrorBound taken. This acts as a cache to avoid waiting for the
    // writer to complete an update and allow to share a reference to this memory location
    // (avoiding some memory copy). Keeping a state here and sharing it with the caller makes the
    // ShmReader not thread safe.
    snapshot_ceb: ClockErrorBound,

    // The value of the writer generation when the ceb snapshot was taken.
    snapshot_gen: u16,
}

impl ShmReader {
    /// Open a ClockBound shared memory segment for reading.
    ///
    /// On error, returns an appropriate `Errno`. If the content of the segment
    /// is uninitialized, unparseable, or otherwise malformed, EPROTO will be
    /// returned.
    pub fn new(path: &CStr) -> Result<ShmReader, ShmError> {
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

        // Move to the end of the header and map the ClockErrorBound data, but only if the segment
        // size allows it and matches our expectation.
        if mmap_guard.segsize < size_of::<ShmHeader>() + size_of::<ClockErrorBound>() {
            return Err(ShmError::SegmentMalformed);
        }
        // SAFETY: segment size has been checked to ensure `cursor` move leads to a valid cast
        cursor = unsafe { cursor.add(size_of::<ShmHeader>()) };
        let ceb_shm = unsafe { ptr::addr_of!(*cursor.cast::<ClockErrorBound>()) };

        Ok(ShmReader {
            _marker: std::marker::PhantomData,
            _guard: mmap_guard,
            version,
            generation,
            ceb_shm,
            snapshot_ceb: ClockErrorBound::default(),
            snapshot_gen: 0,
        })
    }

    /// Return a consistent snapshot of the shared memory segment.
    ///
    /// Taking a snapshot consists in reading the memory segment while confirming the generation
    /// number in the header has not changed (which would indicate an update from the writer
    /// occurred while reading). If an update is detected, the read is retried.
    ///
    /// This function returns a reference to the ClockErrorBound snapshot stored by the reader, and
    /// not an owned value. This make the ShmReader NOT thread-safe: the data pointed to could be
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
            // SAFETY: `ceb_at` has been checked to be valid while creating the ShmReader
            let snapshot = unsafe { self.ceb_shm.read_volatile() };

            // Confirm no update occurred during the read
            let second_gen = generation.load(atomic::Ordering::Acquire);
            if first_gen == second_gen {
                self.snapshot_gen = first_gen;
                self.snapshot_ceb = snapshot;
                return Ok(&self.snapshot_ceb);
            } else {
                // Only track complete updates indicated by an even generation number.
                if second_gen & 0x0001 == 0 {
                    first_gen = second_gen;
                }
            }
            retries -= 1;
        }

        // Attempts to read the snapshot have failed.
        Err(ShmError::SegmentNotInitialized)
    }
}

#[cfg(test)]
mod t_reader {
    /// This test module is full of side effects and create local files to test the ShmHeader
    /// functionality. Tests run concurrently, so each test creates its own dedicated file.
    /// For now, create files in `/tmp/` and no cleaning is done.
    ///
    /// TODO: investigate how to retrieve the target-dir that would work for both brazil package
    /// and "native" cargo ones to contain artefacts better.
    ///
    /// TODO: write more / better tests
    ///
    use super::*;
    use crate::ClockStatus;
    use byteorder::{NativeEndian, WriteBytesExt};
    use std::ffi::CString;
    use std::fs::File;
    use std::io::Write;

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
            let ceb = ClockErrorBound {
                as_of: libc::timespec {
                    tv_sec: $as_of_sec,
                    tv_nsec: $as_of_nsec,
                },
                void_after: libc::timespec {
                    tv_sec: $void_after_sec,
                    tv_nsec: $void_after_nsec,
                },
                bound_nsec: $bound_nsec,
                max_drift_ppb: $max_drift,
                reserved1: 0,
                clock_status: ClockStatus::Synchronized,
            };

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
    fn test_reader_new() {
        let path_shm = "/tmp/test_reader";
        let mut file = File::create(path_shm).expect("create file failed");

        write_memory_segment!(
            file,
            0x414D5A4E,
            0x43420200,
            400,
            3,
            10,
            (0, 0),
            (0, 0),
            123,
            0
        );

        let path = CString::new(path_shm).expect("CString failed");
        let reader = ShmReader::new(&path).expect("Failed to create ShmReader");

        let version = unsafe { &*reader.version };
        let generation = unsafe { &*reader.generation };
        let ceb = unsafe { *reader.ceb_shm };

        assert_eq!(version.load(atomic::Ordering::Relaxed), 3);
        assert_eq!(generation.load(atomic::Ordering::Relaxed), 10);
        assert_eq!(ceb.bound_nsec, 123);
    }
}
