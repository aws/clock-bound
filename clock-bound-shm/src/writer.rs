use byteorder::{NativeEndian, WriteBytesExt};
use std::ffi::{c_void, CString};
use std::io::{Error, ErrorKind};
use std::mem::size_of;
use std::path::Path;
use std::sync::atomic;
use std::{fs, ptr};

use std::io::Seek;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;

use crate::reader::ShmReader;
use crate::shm_header::{ShmHeader, CLOCKBOUND_SHM_SUPPORTED_VERSION, SHM_MAGIC};
use crate::{ClockErrorBound, ShmError};

/// Trait that a writer to the shared memory segment has to implement.
pub trait ShmWrite {
    /// Update the shared memory segment with updated clock error bound data.
    fn write(&mut self, ceb: &ClockErrorBound);
}

/// Writer to the ClockBound shared memory segment.
///
/// This writer is expected to be used by a single process writing to a given path. The file
/// written to is memory mapped by the writer and many (read-only) readers. Updates to the memory
/// segment are applied in a lock-free manner, using a rolling generation number to protect the
/// update section.
#[derive(Debug)]
pub struct ShmWriter {
    /// The size of the segment mapped in memory
    segsize: usize,

    /// A raw pointer keeping the address of the segment mapped in memory
    addr: *mut c_void,

    /// A raw pointer to the version member of the ShmHeader mapped in memory. The version number
    /// identifies the layout of the rest of the segment. A value of 0 indicates the memory segment
    /// is not initialized / not usable.
    version: *mut atomic::AtomicU16,

    /// A raw pointer to the generation member of the ShmHeader mapped in memory. The generation
    /// number is updated by the writer before and after updating the content mapped in memory.
    generation: *mut atomic::AtomicU16,

    /// A raw pointer to the ClockBoundError data mapped in memory. This structure follows the
    /// ShmHeader and contains the information required to compute a bound on clock error.
    ceb: *mut ClockErrorBound,
}

impl ShmWriter {
    /// Create a new ShmWriter referencing the memory segment to write ClockErrorBound data to.
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
    pub fn new(path: &Path) -> std::io::Result<ShmWriter> {
        // Determine the size of the segment.
        let segsize = ShmWriter::segment_size();

        // Use the ShmReader to assert the state of the segment. If the segment does not exist or
        // cannot be read correctly, wipe it clean. Note that there is a strong assumption here
        // that there is only one writer running on the system and writing to `path`. Consequently,
        // it is safe to wipe clean and then update. No-one will attempt to write to the segment
        // even if this process is scheduled out.
        if ShmWriter::is_usable_segment(path).is_err() {
            // Note that wiping the file sets the version to 0, which is used to indicate the
            // readers that the memory segment is not usable yet.
            ShmWriter::wipe(path, segsize)?
        }

        // Memory map the file.
        let addr = ShmWriter::mmap_segment_at(path, segsize)?;

        // Obtain raw pointers to relevant members in the memory map segment and create a new
        // writer.
        // SAFETY: segment has been validated to be usable, can map pointers.
        let (generation, version, ceb) = unsafe {
            let cursor: *mut u8 = addr.cast();
            let generation = ptr::addr_of_mut!((*cursor.cast::<ShmHeader>()).generation);
            let version = ptr::addr_of_mut!((*cursor.cast::<ShmHeader>()).version);
            let ceb: *mut ClockErrorBound = addr.add(size_of::<ShmHeader>()).cast();
            (generation, version, ceb)
        };

        let writer = ShmWriter {
            segsize,
            addr,
            version,
            generation,
            ceb,
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
            let version = &*writer.version;
            version.store(CLOCKBOUND_SHM_SUPPORTED_VERSION, atomic::Ordering::Relaxed);
        }

        Ok(writer)
    }

    /// Check whether the memory segment already exist and is usable.
    ///
    /// The segment is usable if it can be opened at `path` and it can be read by a ShmReader.
    fn is_usable_segment(path: &Path) -> Result<(), ShmError> {
        let path_cstring = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| ShmError::SegmentNotInitialized)?;

        match ShmReader::new(path_cstring.as_c_str()) {
            Ok(_reader) => Ok(()),
            Err(err) => Err(err),
        }
    }

    /// Return a segment size which is large enough to store everything we need.
    fn segment_size() -> usize {
        // Need to hold the header and the bound on clock error data.
        let size = size_of::<ShmHeader>() + size_of::<ClockErrorBound>();

        // Round up to have 64 bit alignment. Not absolutely required but convenient. Currently,
        // the size of the data shared is almost two order of magnitude smaller than the minimum
        // system page size (4096), so taking a quick shortcut and ignoring paging alignment
        // questions for now.
        if size % 8 == 0 {
            size
        } else {
            size + (8 - size % 8)
        }
    }

    /// Initialize the file backing the memory segment.
    ///
    /// Zero out the file up to segsize, but write out header information such the readers can
    /// access it. Note that both the layout version number and the generation number are set to 0,
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

        // Write the ShmHeader
        file.write_u32::<NativeEndian>(SHM_MAGIC[0])?; // Magic number 0
        file.write_u32::<NativeEndian>(SHM_MAGIC[1])?; // Magic number 1
        file.write_u32::<NativeEndian>(size)?; // Segsize
        file.write_u16::<NativeEndian>(0)?; // Version
        file.write_u16::<NativeEndian>(0)?; // Generation

        // Zero the rest of the segment
        let remaining = segsize - size_of::<ShmHeader>();
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

impl ShmWrite for ShmWriter {
    /// Update the clock error bound data in the memory segment.
    ///
    /// This function implements the lock-free mechanism that lets the writer update the memory
    /// segment shared with many readers. The generation number is set to an odd number before the
    /// update and an even number when successfully completed.
    ///
    /// Note that the generation number rolls over, but is never set back to 0, as it would
    /// otherwise signal the readers that the segment is not initialized.
    fn write(&mut self, ceb: &ClockErrorBound) {
        // SAFETY: pointers to fields in the memory segment have been validated on init.
        unsafe {
            // Start by reading the generation value stored in the memory segment.
            let generation = &*self.generation;
            let gen = generation.load(atomic::Ordering::Acquire);

            // Mark the beginning of the update into the memory segment.
            // The producer process may have error'ed or died in the middle of a previous update
            // and left things hanging with an odd generation number. Being a bit fancy, this is
            // our data anti-entropy protection, and make sure we enter the updating section with
            // an odd number.
            let gen = if gen & 0x0001 == 0 {
                // This should be the most common case
                gen.wrapping_add(1)
            } else {
                gen
            };
            generation.store(gen, atomic::Ordering::Release);

            self.ceb.write(*ceb);

            // Mark the end of the update into the memory segment by incrementing the generation
            // number. Note that we skip writing a generation equals to 0 when the counter rolls
            // over. This helps avoid a scenario that would otherwise lead to a bad outcome:
            //   1. the reader reads the generation number which happens to be 0 (just rolled over).
            //   2. the writer resets the memory segment, version and generation are at 0
            //   3. the reader finishes its read, generation is still at 0, but the read is dirty.
            //   4. the writer updates and set version, but it is too late by now.
            //
            // Skipping over a generation equals to 0 avoid this problem.
            let mut gen = gen.wrapping_add(1);
            if gen == 0 {
                gen = 2
            }

            generation.store(gen, atomic::Ordering::Release);
        }
    }
}

impl Drop for ShmWriter {
    /// Unmap the memory segment
    ///
    /// TODO: revisit to see if this can be refactored into the MmapGuard logic implemented on the
    /// ShmReader.
    fn drop(&mut self) {
        unsafe {
            nix::sys::mman::munmap(self.addr, self.segsize).expect("munmap");
        }
    }
}

#[cfg(test)]
mod t_writer {
    use super::*;
    use byteorder::{NativeEndian, ReadBytesExt};
    use nix::sys::time::TimeSpec;
    use std::fs::OpenOptions;
    use std::path::Path;
    /// We make use of tempfile::NamedTempFile to ensure that
    /// local files that are created during a test get removed
    /// afterwards.
    use tempfile::NamedTempFile;

    use crate::ClockStatus;

    macro_rules! clockerrorbound {
        () => {
            ClockErrorBound::new(
                TimeSpec::new(1, 2),       // as_of
                TimeSpec::new(3, 4),       // void_after
                123,                       // bound_nsec
                10,                        // disruption_marker
                100,                       // max_drift_ppb
                ClockStatus::Synchronized, // clock_status
                true,                      // clock_disruption_support_enabled
            )
        };
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

    /// Assert that a new memory mapped segment is created it does not exist.
    #[test]
    fn test_writer_create_new_if_not_exist() {
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        remove_path_if_exists(clockbound_shm_path);

        // Create and wipe the memory segment
        let ceb = clockerrorbound!();
        let mut writer =
            ShmWriter::new(Path::new(clockbound_shm_path)).expect("Failed to create a writer");
        writer.write(&ceb);

        // Read it back into a snapshot
        let path = CString::new(clockbound_shm_path).expect("CString failed");
        let mut reader = ShmReader::new(&path).expect("Failed to create ShmReader");
        let snapshot = reader
            .snapshot()
            .expect("Failed to take a clock error bound snapshot");

        assert_eq!(*snapshot, ceb);
    }

    /// Assert that an existing memory mapped segment is wiped clean if dirty.
    #[test]
    fn test_writer_wipe_clean_on_new() {
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        let mut clockbound_shm_file = OpenOptions::new()
            .write(true)
            .open(clockbound_shm_path)
            .expect("open clockbound file failed");

        // Let's write some garbage first
        let _ = clockbound_shm_file.write(b"foobarbaz");
        let _ = clockbound_shm_file.sync_all();

        // Create and wipe the memory segment
        let ceb = clockerrorbound!();
        let mut writer =
            ShmWriter::new(Path::new(clockbound_shm_path)).expect("Failed to create a writer");
        writer.write(&ceb);

        // Read it back into a snapshot
        let path = CString::new(clockbound_shm_path).expect("CString failed");
        let mut reader = ShmReader::new(&path).expect("Failed to create ShmReader");
        let snapshot = reader
            .snapshot()
            .expect("Failed to take a clock error bound snapshot");

        assert_eq!(*snapshot, ceb);
    }

    /// Assert that an existing and valid segment is reused and updated.
    #[test]
    fn test_writer_update_existing() {
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        remove_path_if_exists(clockbound_shm_path);

        // Create a clean memory segment
        let ceb = clockerrorbound!();
        let mut writer =
            ShmWriter::new(Path::new(clockbound_shm_path)).expect("Failed to create a writer");

        // Push two updates to the shared memory segment, the generation moves from 0, to 2, to 4
        writer.write(&ceb);
        writer.write(&ceb);

        // Check what the writer says
        let generation = unsafe { &*writer.generation };
        let gen = generation.load(atomic::Ordering::Acquire);
        std::mem::drop(writer);
        assert_eq!(gen, 4);

        // Raw validation in the file
        // A bit brittle, would be more robust not to hardcode the seek to the generation field
        let mut file = std::fs::File::open(clockbound_shm_path).expect("create file failed");
        file.seek(std::io::SeekFrom::Start(14))
            .expect("Failed to seek to generation offset");
        let gen = file
            .read_u16::<NativeEndian>()
            .expect("Failed to read generation from file");
        assert_eq!(gen, 4);
    }
}
