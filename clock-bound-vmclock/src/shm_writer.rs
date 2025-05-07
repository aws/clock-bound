use byteorder::{LittleEndian, WriteBytesExt};
use std::ffi::c_void;
use std::io::Seek;
use std::io::Write;
use std::io::{Error, ErrorKind};
use std::mem::size_of;
use std::path::Path;
use std::sync::atomic;
use std::{fs, ptr};

use tracing::debug;

use crate::shm::{VMClockShmBody, VMClockShmHeader, VMCLOCK_SHM_MAGIC};
use crate::shm_reader::VMClockShmReader;
use clock_bound_shm::ShmError;

/// Trait that a writer to the shared memory segment has to implement.
pub trait VMClockShmWrite {
    /// Update the shared memory segment with updated clock error bound data.
    fn write(&mut self, vmclock_shm_body: &VMClockShmBody);
}

/// Writer to the VMClock shared memory segment.
///
/// This writer is expected to be used by a single process writing to a given path. The file
/// written to is memory mapped by the writer and many (read-only) readers. Updates to the memory
/// segment are applied in a lock-free manner, using a rolling seq_count number to protect the
/// update section.
#[derive(Debug)]
pub struct VMClockShmWriter {
    /// The size of the segment mapped in memory
    segsize: usize,

    /// A raw pointer keeping the address of the segment mapped in memory
    addr: *mut c_void,

    /// A raw pointer to the version member of the VMClockShmHeader mapped in memory. The version number
    /// identifies the layout of the rest of the segment. A value of 0 indicates the memory segment
    /// is not initialized / not usable.
    version_ptr: *mut atomic::AtomicU16,

    /// A raw pointer to the sequence count member of the
    /// VMClockShmHeader mapped in memory. The sequence count number is updated by the writer
    /// before and after updating the content mapped in memory.
    seq_count_ptr: *mut atomic::AtomicU32,

    /// A raw pointer to the VMClockShmBody data mapped in memory. This structure follows the
    /// VMClockShmHeader and contains the information required to compute a bound on clock error.
    vmclock_shm_body: *mut VMClockShmBody,
}

impl VMClockShmWriter {
    /// Create a new VMClockShmWriter referencing the memory segment to write VMClockShmBody data to.
    ///
    /// There are several cases to consider:
    /// 1. The file backing the memory segment does not exist, or the content is corrupted/wrong.
    ///    This is a cold start-like scenario, creating a fresh memory mapped file.
    /// 2. The file backing the memory segment already exist and is valid. This may be that the
    ///    process using this writer has restarted, but clients may still be using the existing
    ///    values. Here, we want to load the existing memory segment, and continue as if nothing
    ///    happened. That's a warm reboot-like scenario.
    /// 3. A variation of 2., but where the layout is being changed (a version bump). This is
    ///    analog to a cold boot.
    ///
    /// TODO: implement scenario 3 once the readers support a version bump.
    pub fn new(path: &Path) -> std::io::Result<VMClockShmWriter> {
        // Determine the size of the segment.
        let segsize = VMClockShmWriter::segment_size();

        // Use the VMClockShmReader to assert the state of the segment. If the segment does not exist or
        // cannot be read correctly, wipe it clean. Note that there is a strong assumption here
        // that there is only one writer running on the system and writing to `path`. Consequently,
        // it is safe to wipe clean and then update. No-one will attempt to write to the segment
        // even if this process is scheduled out.
        if VMClockShmWriter::is_usable_segment(path).is_err() {
            // Note that wiping the file sets the version to 0, which is used to indicate the
            // readers that the memory segment is not usable yet.
            VMClockShmWriter::wipe(path, segsize)?
        }

        // Memory map the file.
        let addr = VMClockShmWriter::mmap_segment_at(path, segsize)?;

        // Obtain raw pointers to relevant members in the memory map segment and create a new
        // writer.
        // SAFETY: segment has been validated to be usable, can map pointers.
        //
        let (version_ptr, seq_count_ptr, vmclock_shm_body) = unsafe {
            let cursor: *mut u8 = addr.cast();
            let version_ptr = ptr::addr_of_mut!((*cursor.cast::<VMClockShmHeader>()).version);
            let seq_count_ptr = ptr::addr_of_mut!((*cursor.cast::<VMClockShmHeader>()).seq_count);
            let vmclock_shm_body: *mut VMClockShmBody =
                addr.add(size_of::<VMClockShmHeader>()).cast();
            (version_ptr, seq_count_ptr, vmclock_shm_body)
        };

        let writer = VMClockShmWriter {
            segsize,
            addr,
            version_ptr,
            seq_count_ptr,
            vmclock_shm_body,
        };

        // Update the memory segment with bound on clock error data and write the layout version.
        // - If the segment was wiped clean, this defines the memory layout. It is still not useable
        //   by readers, until the next `update()` is successful.
        // - If the segment existed and was valid, the version is over-written, and with a single
        //   version defined today, this overwrites the same value and the segment is readily
        //   available to the existing readers.
        //
        // TODO: remove the hard coded version 1 below, manage a change of version, and update the
        // comment above since the no-op assumption won't hold true anymore with more than one
        // version.
        // SAFETY: segment has been validated to be usable, can use pointers.
        unsafe {
            let version_number = 1_u16;
            let version = &*writer.version_ptr;
            version.store(version_number, atomic::Ordering::Relaxed);
        }

        Ok(writer)
    }

    /// Check whether the memory segment already exist and is usable.
    ///
    /// The segment is usable if it can be opened at `path` and it can be read by a VMClockShmReader.
    fn is_usable_segment(path: &Path) -> Result<(), ShmError> {
        if let Some(path_str) = path.to_str() {
            match VMClockShmReader::new(path_str) {
                Ok(_reader) => Ok(()),
                Err(err) => Err(err),
            }
        } else {
            Err(ShmError::SegmentNotInitialized)
        }
    }

    /// Return a segment size which is large enough to store everything we need.
    fn segment_size() -> usize {
        // Need to hold the header and the bound on clock error data.
        let size = size_of::<VMClockShmHeader>() + size_of::<VMClockShmBody>();
        debug!("Segment size (header + body) is {:?}", size);
        size
    }

    /// Initialize the file backing the memory segment.
    ///
    /// Zero out the file up to segsize, but write out header information such the readers can
    /// access it. Note that both the layout version number and the seq_count number are set to 0,
    /// which makes this file not usable to retrieve clock error bound data yet.
    fn wipe(path: &Path, segsize: usize) -> std::io::Result<()> {
        // Attempt at creating intermediate directories, but do expect that the base permissions
        // are set correctly.
        if let Some(parent) = path.parent() {
            match parent.to_str() {
                Some("") => (), // This would be a relative path without parent
                Some(_) => fs::create_dir_all(parent)?,
                None => {
                    return Err(Error::new(
                        ErrorKind::Other,
                        "Failed to extract parent dir name",
                    ))
                }
            }
        }

        // Opens the file in write-only mode. Create a file if it does not exist, and truncate it
        // if it does.
        let mut file = std::fs::File::create(path)?;

        // In theory, usize may not fit within a u32. In practice, we
        let size: u32 = match segsize.try_into() {
            Ok(size) => size, // it did fit
            Err(e) => {
                return Err(std::io::Error::new(
                    ErrorKind::Other,
                    format!(
                        "Failed to convert segment size {:?} into u32 {:?}",
                        segsize, e
                    ),
                ))
            }
        };

        // Write the VMClockShmHeader

        // Magic number.
        file.write_u32::<LittleEndian>(VMCLOCK_SHM_MAGIC)?;
        // Segsize.
        file.write_u32::<LittleEndian>(size)?;
        // Version.
        file.write_u16::<LittleEndian>(0_u16)?;
        // Counter ID.
        file.write_u8(0_u8)?;
        // Time type.
        file.write_u8(0_u8)?;
        // Sequence count.
        file.write_u32::<LittleEndian>(0_u32)?;

        // Zero the rest of the segment
        let remaining = segsize - size_of::<VMClockShmHeader>();
        let buf = vec![0; remaining];
        file.write_all(&buf)?;

        // Make sure the amount of bytes written matches the segment size
        let pos = file.stream_position()?;
        if pos > size.into() {
            return Err(std::io::Error::new(
                ErrorKind::Other,
                format!(
                    "SHM Writer implementation error: wrote {:?} bytes but segsize is {:?} bytes",
                    pos, size
                ),
            ));
        }

        // Sync all and drop (close) the descriptor
        file.sync_all()?;

        Ok(())
    }

    /// Open and map the file at the given path to memory.
    ///
    /// TODO: implementation is using the nix crate, but may want to revisit and see if it would be
    /// worth refactoring to align with the reader implementation, which is similar (but
    /// read-only).
    fn mmap_segment_at(path: &Path, segsize: usize) -> std::io::Result<*mut c_void> {
        let fd = nix::fcntl::open(
            path,
            nix::fcntl::OFlag::O_RDWR,
            nix::sys::stat::Mode::from_bits_truncate(0o644),
        )?;

        // SAFETY: always safe when addr is None.
        unsafe {
            nix::sys::mman::mmap(
                None,
                std::num::NonZeroUsize::new(segsize).unwrap(),
                nix::sys::mman::ProtFlags::PROT_READ | nix::sys::mman::ProtFlags::PROT_WRITE,
                nix::sys::mman::MapFlags::MAP_SHARED,
                fd,
                0,
            )
            .map_err(|e| {
                let _ = nix::unistd::close(fd);
                e.into()
            })
        }
    }
}

impl VMClockShmWrite for VMClockShmWriter {
    /// Update the clock error bound data in the memory segment.
    ///
    /// This function implements the lock-free mechanism that lets the writer update the memory
    /// segment shared with many readers. The seq_count number is set to an odd number before the
    /// update and an even number when successfully completed.
    ///
    fn write(&mut self, vmclock_shm_body: &VMClockShmBody) {
        // SAFETY: pointers to fields in the memory segment have been validated on init.
        unsafe {
            // Start by reading the seq_count value stored in the memory segment.
            let seq_count = &*self.seq_count_ptr;
            let seq_count_value = seq_count.load(atomic::Ordering::Acquire);

            // Mark the beginning of the update into the memory segment.
            // The producer process may have error'ed or died in the middle of a previous update
            // and left things hanging with an odd seq_count number. Being a bit fancy, this is
            // our data anti-entropy protection, and make sure we enter the updating section with
            // an odd number.
            let seq_count_value = if seq_count_value & 0x0001 == 0 {
                // This should be the most common case
                seq_count_value.wrapping_add(1)
            } else {
                seq_count_value
            };
            seq_count.store(seq_count_value, atomic::Ordering::Release);

            self.vmclock_shm_body.write(*vmclock_shm_body);
            let seq_count_value = seq_count_value.wrapping_add(1);
            seq_count.store(seq_count_value, atomic::Ordering::Release);
        }
    }
}

impl Drop for VMClockShmWriter {
    /// Unmap the memory segment
    ///
    /// TODO: revisit to see if this can be refactored into the MmapGuard logic implemented on the
    /// VMClockShmReader.
    fn drop(&mut self) {
        unsafe {
            nix::sys::mman::munmap(self.addr, self.segsize).expect("munmap");
        }
    }
}

#[cfg(test)]
mod t_writer {
    use super::*;
    use byteorder::{LittleEndian, ReadBytesExt};
    use std::path::Path;
    /// We make use of tempfile::NamedTempFile to ensure that
    /// local files that are created during a test get removed
    /// afterwards.
    use tempfile::NamedTempFile;

    use crate::shm::VMClockClockStatus;

    macro_rules! vmclockshmbody {
        () => {
            VMClockShmBody {
                disruption_marker: 0,
                flags: 0,
                _padding: [0x00, 0x00],
                clock_status: VMClockClockStatus::Unknown,
                leap_second_smearing_hint: 0,
                tai_offset_sec: 0,
                leap_indicator: 0,
                counter_period_shift: 0,
                counter_value: 0,
                counter_period_frac_sec: 0,
                counter_period_esterror_rate_frac_sec: 0,
                counter_period_maxerror_rate_frac_sec: 0,
                time_sec: 0,
                time_frac_sec: 0,
                time_esterror_nanosec: 0,
                time_maxerror_nanosec: 0,
            }
        };
    }

    /// Helper function to remove files created during unit tests.
    fn remove_file_or_directory(path: &str) {
        // Busy looping on deleting the previous file, good enough for unit test
        let p = Path::new(&path);
        while p.exists() {
            if p.is_dir() {
                std::fs::remove_dir_all(&path).expect("failed to remove file");
            } else {
                std::fs::remove_file(&path).expect("failed to remove file");
            }
        }
    }

    #[test]
    fn test_segment_size() {
        let actual_segment_size = VMClockShmWriter::segment_size();
        let expected_segment_size = 104_usize;
        assert_eq!(actual_segment_size, expected_segment_size);
    }

    /// Assert that a new memory mapped segment is created it does not exist.
    #[test]
    fn test_writer_create_new_if_not_exist() {
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        remove_file_or_directory(&vmclock_shm_path);

        // Create and wipe the memory segment
        let vmclock_shm_body = vmclockshmbody!();
        let mut writer =
            VMClockShmWriter::new(Path::new(&vmclock_shm_path)).expect("Failed to create a writer");
        writer.write(&vmclock_shm_body);

        // Read it back into a snapshot
        let mut reader =
            VMClockShmReader::new(&vmclock_shm_path).expect("Failed to create VMClockShmReader");
        let snapshot = reader
            .snapshot()
            .expect("Failed to take a VMClockShmBody snapshot");

        assert_eq!(*snapshot, vmclock_shm_body);
    }

    /// Assert that an existing memory mapped segment is wiped clean if dirty.
    #[test]
    fn test_writer_wipe_clean_on_new() {
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        remove_file_or_directory(&vmclock_shm_path);

        // Let's write some garbage first
        let mut file = std::fs::File::create(&vmclock_shm_path).expect("create file failed");
        let _ = file.write(b"foobarbaz");
        let _ = file.sync_all();

        // Create and wipe the memory segment
        let vmclock_shm_body = vmclockshmbody!();
        let mut writer =
            VMClockShmWriter::new(Path::new(&vmclock_shm_path)).expect("Failed to create a writer");
        writer.write(&vmclock_shm_body);

        // Read it back into a snapshot
        let mut reader =
            VMClockShmReader::new(&vmclock_shm_path).expect("Failed to create VMClockShmReader");
        let snapshot = reader
            .snapshot()
            .expect("Failed to take a clock error bound snapshot");

        assert_eq!(*snapshot, vmclock_shm_body);
    }

    /// Assert that an existing and valid segment is reused and updated.
    #[test]
    fn test_writer_update_existing() {
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        remove_file_or_directory(&vmclock_shm_path);

        // Create a clean memory segment
        let vmclock_shm_body = vmclockshmbody!();
        let mut writer =
            VMClockShmWriter::new(Path::new(&vmclock_shm_path)).expect("Failed to create a writer");

        // Push two updates to the shared memory segment, the seq_count moves from 0, to 2, to 4
        writer.write(&vmclock_shm_body);
        writer.write(&vmclock_shm_body);

        // Check what the writer says
        let seq_count = unsafe { &*writer.seq_count_ptr };
        let seq_count_value = seq_count.load(atomic::Ordering::Acquire);
        std::mem::drop(writer);
        assert_eq!(seq_count_value, 4);

        // Raw validation in the file
        // A bit brittle, would be more robust not to hardcode the seek to the seq_count field
        let mut file = std::fs::File::open(&vmclock_shm_path).expect("create file failed");
        file.seek(std::io::SeekFrom::Start(12))
            .expect("Failed to seek to seq_count offset");
        let seq_count_value = file
            .read_u64::<LittleEndian>()
            .expect("Failed to read seq_count from file");
        assert_eq!(seq_count_value, 4);
    }
}
