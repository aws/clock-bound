use std::ffi::c_void;
use std::fs::File;
use std::io::Read;
use std::mem::size_of;
use std::os::fd::AsRawFd;
use std::ptr;
use std::sync::atomic;
use tracing::{debug, error};

use crate::shm::{VMClockShmBody, VMClockShmHeader};
use clock_bound_shm::{syserror, ShmError};

const VMCLOCK_SUPPORTED_VERSION: u16 = 1;

/// A guard tracking an memory mapped file.
///
/// Creating the MmapGuard maps an open file descriptor.
/// The file is unmap'ed when the guard is dropped.
struct MmapGuard {
    /// A pointer to the head of the segment
    segment: *mut c_void,

    /// The size of the segment mapped into memory
    segsize: usize,

    /// Memory mapped file.
    _file: File,
}

impl MmapGuard {
    /// Create a new MmapGuard.
    ///
    /// Memory map the provided open File.
    fn new(mut file: File) -> Result<Self, ShmError> {
        let mut buffer = vec![];

        let bytes_read = match file.read_to_end(&mut buffer) {
            Ok(bytes_read) => bytes_read,
            Err(_) => return syserror!("Failed to read SHM segment"),
        };

        if bytes_read == 0_usize {
            error!("MmapGuard: Read zero bytes.");
            return Err(ShmError::SegmentNotInitialized);
        } else if bytes_read < size_of::<VMClockShmHeader>() {
            error!("MmapGuard: Number of bytes read ({:?}) is less than the size of VMClockShmHeader ({:?}).", bytes_read, size_of::<VMClockShmHeader>());
            return Err(ShmError::SegmentMalformed);
        }

        debug!("MMapGuard: Reading the VMClockShmHeader ...");

        // Read the header so we know how much to map in memory.
        let header = VMClockShmHeader::read(&buffer)?;

        // This consumes the segsize, but we only needed the header for validation and extracting
        // the segment size. So the move is fine here.
        let segsize = header.size.into_inner() as usize;

        debug!("MMapGuard: Read a segment size of: {:?}", segsize);

        // SAFETY: We're calling into a C function, but this particular call is always safe.
        let segment: *mut c_void = unsafe {
            libc::mmap(
                ptr::null_mut(),
                segsize,
                libc::PROT_READ,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };

        if segment == libc::MAP_FAILED {
            return syserror!("mmap SHM segment");
        }

        Ok(MmapGuard {
            segment,
            segsize,
            _file: file,
        })
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

/// Reader for VMClock shared memory segment.
///
/// The VMClock shared memory segment consists of a VMClockShmHeader followed by a
/// VMClockShmBody struct. The segment is updated by a single producer (the Hypervisor),
/// but may be read by many clients.  The shared memory segment does not implement a semaphore or
/// equivalent to synchronize the single-producer / many-consumers processes. Instead, the
/// mechanism is lock-free and relies on a `seq_count` number to ensure consistent reads (over
/// retries).
///
/// The writer increments the seq_count field from even to odd before each update. It also
/// increment it again, from odd to even, after finishing the update. Readers must check the
/// `seq_count` field before and after each read, and verify that they obtain the same, even,
/// value. Otherwise, the read was dirty and must be retried.
pub struct VMClockShmReader {
    // Explicitly make the VMClockShmReader be !Send and !Sync, since it is not thread safe. A bit ugly to
    // use a phantom raw pointer, but effective and free at runtime.
    _marker: std::marker::PhantomData<*const ()>,

    // Drop guard to unmap the shared memory segment
    _guard: MmapGuard,

    // A raw pointer into the shared memory segment, pointing to the version member of the VMClockShmHeader
    // section. The version number defines the shared memory segment content and layout. This is a
    // bit less flexible than a series of TLV but simpler (and not mutually exclusive).
    version_ptr: *const atomic::AtomicU16,

    // A raw pointer into the shared memory segment, pointing to the seq_count member of the
    // VMClockShmHeader section. The seq_count is used to read consistent snapshots of the shared
    // memory segment (that is outside of an update event by the writer). This is expected to roll
    // over as a function of the rate of update from the writer (eg. every ~9 hours if updating
    // once a second).
    seq_count_ptr: *const atomic::AtomicU32,

    // A raw pointer into the shared memory segment, pointing to the VMClockShmBody section. Note
    // that the structured reference by this pointer may not be consistent, and reading it requires
    // to assert the seq_count value.
    vmclock_shm_body_ptr: *const VMClockShmBody,

    // The last snapshot of VMClockShmBody taken. This acts as a cache to avoid waiting for the
    // writer to complete an update and allow to share a reference to this memory location
    // (avoiding some memory copy). Keeping a state here and sharing it with the caller makes the
    // VMClockShmReader not thread safe.
    vmclock_shm_body_snapshot: VMClockShmBody,

    // The value of seq_count when the VMClockShmBody snapshot was taken.
    seq_count_snapshot: u32,
}

impl VMClockShmReader {
    /// Open a VMClock shared memory segment for reading.
    ///
    /// On error, returns an appropriate `Errno`. If the content of the segment
    /// is uninitialized, unparseable, or otherwise malformed, EPROTO will be
    /// returned.
    pub fn new(path: &str) -> Result<VMClockShmReader, ShmError> {
        debug!("VMClockShmReader::new(): path is: {:?}", path);
        let file = match File::open(path) {
            Ok(f) => f,
            Err(e) => {
                error!("VMClockShmReader::new(): {:?}", e);
                return Err(ShmError::SegmentNotInitialized);
            }
        };

        debug!("VMClockShmReader::new(): Creating a MmapGuard ...");
        let mmap_guard = MmapGuard::new(file)?;

        // Create a cursor to pick the addresses of the various elements of interest in the shared
        // memory segment.
        let mut cursor: *const u8 = mmap_guard.segment.cast();
        debug!("VMClockShmReader::new(): Created the cursor.");

        // Pick fields from the VMClockShmHeader
        // SAFETY: `cursor` is aligned to the start of the memory segment and the MmapGuard has
        // validated the memory segment is large enough to contain the header.

        let version_ptr = unsafe { ptr::addr_of!((*cursor.cast::<VMClockShmHeader>()).version) };
        let counter_id_ptr =
            unsafe { ptr::addr_of!((*cursor.cast::<VMClockShmHeader>()).counter_id) };
        let time_type_ptr =
            unsafe { ptr::addr_of!((*cursor.cast::<VMClockShmHeader>()).time_type) };
        let seq_count_ptr =
            unsafe { ptr::addr_of!((*cursor.cast::<VMClockShmHeader>()).seq_count) };

        // Validate that the VMClock shared memory segment has a version number that is supported
        // by this reader.
        // SAFETY: MmapGuard has validated the memory segment of the header. `version_ptr` points
        // to a part of the memory segment in this header.
        let version = unsafe { &*version_ptr };
        let version_number = version.load(atomic::Ordering::Acquire);
        if version_number != VMCLOCK_SUPPORTED_VERSION {
            error!("VMClock shared memory segment has version {:?} which is not supported by this version of the VMClockShmReader.", version_number);
            return Err(ShmError::SegmentVersionNotSupported);
        }

        // Log the counter_id in the shared memory segment.
        // SAFETY: MmapGuard has validated the memory segment of the header. `counter_id_ptr` points
        // to a part of the memory segment in this header.
        let counter_id = unsafe { &*counter_id_ptr };
        let counter_id_value = counter_id.load(atomic::Ordering::Acquire);
        debug!("VMClockShmReader::(): counter_id: {:?}", counter_id_value);

        // Log the time_type in the shared memory segment.
        // SAFETY: MmapGuard has validated the memory segment of the header. `time_type_ptr` points
        // to a part of the memory segment in this header.
        let time_type = unsafe { &*time_type_ptr };
        let time_type_value = time_type.load(atomic::Ordering::Acquire);
        debug!("VMClockShmReader::(): time_type: {:?}", time_type_value);

        // Move to the end of the header and map the VMClockShmBody data, but only if the segment
        // size allows it and matches our expectation.
        if mmap_guard.segsize < (size_of::<VMClockShmHeader>() + size_of::<VMClockShmBody>()) {
            error!("VMClockShmReader::new(): Segment size is smaller than expected.");
            return Err(ShmError::SegmentMalformed);
        }
        // SAFETY: segment size has been checked to ensure `cursor` move leads to a valid cast
        cursor = unsafe { cursor.add(size_of::<VMClockShmHeader>()) };
        let vmclock_shm_body_ptr = unsafe { ptr::addr_of!(*cursor.cast::<VMClockShmBody>()) };

        Ok(VMClockShmReader {
            _marker: std::marker::PhantomData,
            _guard: mmap_guard,
            version_ptr,
            seq_count_ptr,
            vmclock_shm_body_ptr,
            vmclock_shm_body_snapshot: VMClockShmBody::default(),
            seq_count_snapshot: 0,
        })
    }

    /// Return a consistent snapshot of the shared memory segment.
    ///
    /// Taking a snapshot consists in reading the memory segment while confirming the seq_count
    /// number in the header has not changed (which would indicate an update from the writer
    /// occurred while reading). If an update is detected, the read is retried.
    ///
    /// This function returns a reference to the VMClockShmBody snapshot stored by the reader, and
    /// not an owned value. This make the VMClockShmReader NOT thread-safe: the data pointed to could be
    /// updated without one of the thread knowing, leading to a incorrect clock error bond. The
    /// advantage are in terms of performance: less data copied, but also no locking, yielding or
    /// excessive retries.
    pub fn snapshot(&mut self) -> Result<&VMClockShmBody, ShmError> {
        // Atomically read the current version in the shared memory segment
        // SAFETY: `self.version` has been validated when creating the reader
        let version = unsafe { &*self.version_ptr };
        let version = version.load(atomic::Ordering::Acquire);

        // Validate version number.
        //
        // We are validating the version prior to each snapshot to protect
        // against a Hypervisor which has implemented an unsupported VMClock version.
        if version != VMCLOCK_SUPPORTED_VERSION {
            error!("VMClock shared memory segment has version {:?} which is not supported by this version of the VMClockShmReader.", version);
            return Err(ShmError::SegmentVersionNotSupported);
        }

        // Atomically read the current seq_count in the shared memory segment
        // SAFETY: `self.seq_count_ptr` has been validated when creating the reader
        let seq_count = unsafe { &*self.seq_count_ptr };
        let mut seq_count_first = seq_count.load(atomic::Ordering::Acquire);

        // Quick optimization, if the seq_count number matches the last one recorded, the shared
        // memory segment has not been updated since last read. No need to read more of the memory
        // segment, instead return the reference to the snapshot. This is useful in cases where the
        // rate of clockbound read is much higher than the rate of write to the shared memory
        // segment.
        //
        // Although the seq_count number could theoretically roll over, it is unlikely to do so
        // over the lifespan of the instance running.
        // Assuming an update on every jiffy with the largest Linux HZ value of 1000,
        // updates would occur every 1 millisecond.  Therefore a roll-over would occur
        // approximately after: 1 millisecond * ((2 ^ 64) - 1) = 584542046.0906265 years.
        //
        if seq_count_first == self.seq_count_snapshot {
            return Ok(&self.vmclock_shm_body_snapshot);
        }

        // The seq_count number has changed since the last snapshot. Loop
        // until we obtain a consistent read of the clock error bound data. This relies on reading
        // the seq_count value twice, making sure they are identical and an even number.
        //
        // The writer of the VMClock in the production environment is expected to be the
        // Hypervisor.  It is not expected to die, but we do cap the number of retries in case
        // there is an unexpected bug in the Hypervisor.
        let mut retries = u32::MAX;
        while retries > 0 {
            // Read the VMClockShmBody data from the shared memory
            // SAFETY: `VMClockShmBody` has been checked to be valid while creating the VMClockShmReader
            let snapshot = unsafe { self.vmclock_shm_body_ptr.read_volatile() };

            // Confirm no update occurred during the read
            let seq_count_second = seq_count.load(atomic::Ordering::Acquire);

            // Only track complete updates indicated by an even seq_count number.
            if seq_count_second & 0x0001 == 0 {
                if seq_count_first == seq_count_second {
                    self.seq_count_snapshot = seq_count_first;
                    self.vmclock_shm_body_snapshot = snapshot;
                    return Ok(&self.vmclock_shm_body_snapshot);
                } else {
                    seq_count_first = seq_count_second;
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
    use super::*;
    use crate::shm::VMClockClockStatus;
    use std::fs::{File, OpenOptions};
    use std::io::Write;
    use std::path::Path;
    /// We make use of tempfile::NamedTempFile to ensure that
    /// local files that are created during a test get removed
    /// afterwards.
    use tempfile::NamedTempFile;

    /// Test struct used to hold the expected fields in the VMClock shared memory segment.
    #[repr(C)]
    #[derive(Debug, Copy, Clone, PartialEq)]
    struct VMClockContent {
        magic: u32,
        size: u32,
        version: u16,
        counter_id: u8,
        time_type: u8,
        seq_count: u32,
        disruption_marker: u64,
        flags: u64,
        _padding: [u8; 2],
        clock_status: VMClockClockStatus,
        leap_second_smearing_hint: u8,
        tai_offset_sec: i16,
        leap_indicator: u8,
        counter_period_shift: u8,
        counter_value: u64,
        counter_period_frac_sec: u64,
        counter_period_esterror_rate_frac_sec: u64,
        counter_period_maxerror_rate_frac_sec: u64,
        time_sec: u64,
        time_frac_sec: u64,
        time_esterror_nanosec: u64,
        time_maxerror_nanosec: u64,
    }

    fn write_vmclock_content(file: &mut File, vmclock_content: &VMClockContent) {
        // Convert the VMClockShmBody struct into a slice so we can write it all out, fairly magic.
        // Definitely needs the #[repr(C)] layout.
        let slice = unsafe {
            ::core::slice::from_raw_parts(
                (vmclock_content as *const VMClockContent) as *const u8,
                ::core::mem::size_of::<VMClockContent>(),
            )
        };

        file.write_all(slice).expect("Write failed VMClockContent");
        file.sync_all().expect("Sync to disk failed");
    }

    fn remove_path_if_exists(path_shm: &str) {
        let path = Path::new(path_shm);
        if path.exists() {
            if path.is_dir() {
                std::fs::remove_dir_all(path_shm).expect("failed to remove file");
            } else {
                std::fs::remove_file(path_shm).expect("failed to remove file");
            }
        }
    }

    /// Assert that the reader can map a file.
    #[test]
    fn test_reader_new() {
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        let mut vmclock_shm_file = OpenOptions::new()
            .write(true)
            .open(vmclock_shm_path)
            .expect("open vmclock file failed");
        let vmclock_content = VMClockContent {
            magic: 0x4B4C4356,
            size: 104_u32,
            version: 1_u16,
            counter_id: 1_u8,
            time_type: 0_u8,
            seq_count: 10_u32,
            disruption_marker: 888888_u64,
            flags: 0_u64,
            _padding: [0x00, 0x00],
            clock_status: VMClockClockStatus::Synchronized,
            leap_second_smearing_hint: 0_u8,
            tai_offset_sec: 0_i16,
            leap_indicator: 0_u8,
            counter_period_shift: 0_u8,
            counter_value: 123456_u64,
            counter_period_frac_sec: 0_u64,
            counter_period_esterror_rate_frac_sec: 0_u64,
            counter_period_maxerror_rate_frac_sec: 0_u64,
            time_sec: 0_u64,
            time_frac_sec: 0_u64,
            time_esterror_nanosec: 0_u64,
            time_maxerror_nanosec: 0_u64,
        };
        write_vmclock_content(&mut vmclock_shm_file, &vmclock_content);

        let reader =
            VMClockShmReader::new(&vmclock_shm_path).expect("Failed to create VMClockShmReader");

        let version = unsafe { &*reader.version_ptr };
        let seq_count = unsafe { &*reader.seq_count_ptr };
        let vmclock_shm_body = unsafe { *reader.vmclock_shm_body_ptr };

        assert_eq!(version.load(atomic::Ordering::Relaxed), 1_u16);
        assert_eq!(seq_count.load(atomic::Ordering::Relaxed), 10_u32);
        assert_eq!(vmclock_shm_body.counter_value, 123456_u64);
        assert_eq!(
            vmclock_shm_body.clock_status,
            VMClockClockStatus::Synchronized
        );
        assert_eq!(vmclock_shm_body.disruption_marker, 888888_u64);
    }

    /// Assert that the reader will return an error when it tries to open a file that does not exist.
    #[test]
    fn test_reader_file_does_not_exist() {
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        remove_path_if_exists(vmclock_shm_path);

        let expected = ShmError::SegmentNotInitialized;
        match VMClockShmReader::new(&vmclock_shm_path) {
            Err(actual) => assert_eq!(expected, actual),
            _ => assert!(false),
        }
    }

    /// Assert that the reader will return an error when it tries to open a file that is empty.
    #[test]
    fn test_reader_file_is_empty() {
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();

        let expected = ShmError::SegmentNotInitialized;
        match VMClockShmReader::new(&vmclock_shm_path) {
            Err(actual) => assert_eq!(expected, actual),
            _ => assert!(false),
        }
    }

    /// Assert that the reader will return an error when it tries to read a file
    /// that has an unsupported VMClock version.
    #[test]
    fn test_reader_version_not_supported() {
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        let mut vmclock_shm_file = OpenOptions::new()
            .write(true)
            .open(vmclock_shm_path)
            .expect("open vmclock file failed");
        let vmclock_content = VMClockContent {
            magic: 0x4B4C4356,
            size: 104_u32,
            version: 999_u16,
            counter_id: 1_u8,
            time_type: 0_u8,
            seq_count: 10_u32,
            disruption_marker: 888888_u64,
            flags: 0_u64,
            _padding: [0x00, 0x00],
            clock_status: VMClockClockStatus::Synchronized,
            leap_second_smearing_hint: 0_u8,
            tai_offset_sec: 0_i16,
            leap_indicator: 0_u8,
            counter_period_shift: 0_u8,
            counter_value: 123456_u64,
            counter_period_frac_sec: 0_u64,
            counter_period_esterror_rate_frac_sec: 0_u64,
            counter_period_maxerror_rate_frac_sec: 0_u64,
            time_sec: 0_u64,
            time_frac_sec: 0_u64,
            time_esterror_nanosec: 0_u64,
            time_maxerror_nanosec: 0_u64,
        };
        write_vmclock_content(&mut vmclock_shm_file, &vmclock_content);

        let expected = ShmError::SegmentVersionNotSupported;
        match VMClockShmReader::new(&vmclock_shm_path) {
            Err(actual) => assert_eq!(expected, actual),
            _ => assert!(false),
        }
    }

    /// Assert that the reader will return an error when it tries to read a file
    /// that has a segement size that is too small to fit the VMClockShmHeader.
    #[test]
    fn test_reader_segment_size_smaller_than_header() {
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        let mut vmclock_shm_file = OpenOptions::new()
            .write(true)
            .open(vmclock_shm_path)
            .expect("open vmclock file failed");
        let vmclock_content = VMClockContent {
            magic: 0x4B4C4356,
            size: 8_u32, // Writing a size smaller than the header size of 16.
            version: 1_u16,
            counter_id: 1_u8,
            time_type: 0_u8,
            seq_count: 10_u32,
            disruption_marker: 888888_u64,
            flags: 0_u64,
            _padding: [0x00, 0x00],
            clock_status: VMClockClockStatus::Synchronized,
            leap_second_smearing_hint: 0_u8,
            tai_offset_sec: 0_i16,
            leap_indicator: 0_u8,
            counter_period_shift: 0_u8,
            counter_value: 123456_u64,
            counter_period_frac_sec: 0_u64,
            counter_period_esterror_rate_frac_sec: 0_u64,
            counter_period_maxerror_rate_frac_sec: 0_u64,
            time_sec: 0_u64,
            time_frac_sec: 0_u64,
            time_esterror_nanosec: 0_u64,
            time_maxerror_nanosec: 0_u64,
        };
        write_vmclock_content(&mut vmclock_shm_file, &vmclock_content);

        let expected = ShmError::SegmentMalformed;
        match VMClockShmReader::new(&vmclock_shm_path) {
            Err(actual) => assert_eq!(expected, actual),
            _ => assert!(false),
        }
    }

    /// Assert that the reader will return an error when it tries to read a file
    /// that has a segement size that is large enough for the VMClockShmHeader,
    /// but not large enough for both the VMClockShmHeader and VMClockShmBody.
    #[test]
    fn test_reader_segment_size_smaller_than_header_and_body() {
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        let mut vmclock_shm_file = OpenOptions::new()
            .write(true)
            .open(vmclock_shm_path)
            .expect("open vmclock file failed");
        let vmclock_content = VMClockContent {
            magic: 0x4B4C4356,
            size: 26_u32, // Writing a size slightly larger than the header size of 16.
            version: 1_u16,
            counter_id: 1_u8,
            time_type: 0_u8,
            seq_count: 10_u32,
            disruption_marker: 888888_u64,
            flags: 0_u64,
            _padding: [0x00, 0x00],
            clock_status: VMClockClockStatus::Synchronized,
            leap_second_smearing_hint: 0_u8,
            tai_offset_sec: 0_i16,
            leap_indicator: 0_u8,
            counter_period_shift: 0_u8,
            counter_value: 123456_u64,
            counter_period_frac_sec: 0_u64,
            counter_period_esterror_rate_frac_sec: 0_u64,
            counter_period_maxerror_rate_frac_sec: 0_u64,
            time_sec: 0_u64,
            time_frac_sec: 0_u64,
            time_esterror_nanosec: 0_u64,
            time_maxerror_nanosec: 0_u64,
        };
        write_vmclock_content(&mut vmclock_shm_file, &vmclock_content);

        let expected = ShmError::SegmentMalformed;
        match VMClockShmReader::new(&vmclock_shm_path) {
            Err(actual) => assert_eq!(expected, actual),
            _ => assert!(false),
        }
    }
}
