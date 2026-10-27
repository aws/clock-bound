#![expect(clippy::cast_ptr_alignment, reason = "TODO COME BACK TO THIS")]

use byteorder::{NativeEndian, WriteBytesExt};
use std::ffi::{CString, c_void};
use std::io::Error;
use std::mem::size_of;
use std::path::Path;
use std::sync::atomic;
use std::{fs, ptr};

use std::io::Seek;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;

use crate::shm::reader::ShmReader;
use crate::shm::shm_header::{CLOCKBOUND_SHM_LATEST_VERSION, SHM_MAGIC, ShmHeader};
use crate::shm::{
    ClockErrorBound, ClockErrorBoundLayoutVersion, ClockErrorBoundV2, ClockErrorBoundV3, ShmError,
};

/// Typed pointer to the inner `#[repr(C)]` payload stored in the shared
/// memory segment. The segment holds the raw payload struct (never the
/// `ClockErrorBound` enum, which has no stable layout), so the writer must
/// serialize through the variant matching the segment's layout version.
#[derive(Debug)]
enum LayoutPtr {
    V2(*mut ClockErrorBoundV2),
    V3(*mut ClockErrorBoundV3),
}

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

    /// A raw pointer to the version member of the `ShmHeader` mapped in memory. The version number
    /// identifies the layout of the rest of the segment. A value of 0 indicates the memory segment
    /// is not initialized / not usable.
    version: *mut atomic::AtomicU16,

    /// A raw pointer to the generation member of the `ShmHeader` mapped in memory. The generation
    /// number is updated by the writer before and after updating the content mapped in memory.
    generation: *mut atomic::AtomicU16,

    /// A typed pointer to the inner `#[repr(C)]` payload mapped in memory. The payload follows the
    /// `ShmHeader` and contains the information required to compute a bound on clock error. The
    /// variant is selected by the segment's layout version so the writer never serializes the
    /// layout-unstable `ClockErrorBound` enum into the segment.
    ceb: LayoutPtr,
}

// SAFETY: This struct holds exclusive access to the raw memory mapped pointers and
// can be sent to other threads. No limitations from mmap
unsafe impl Send for ShmWriter {}

// SAFETY: It is Sync because we do not write to the underlying pointers with `&self`.
// All mutations require `&mut self`
unsafe impl Sync for ShmWriter {}

impl ShmWriter {
    /// Create a new `ShmWriter` referencing the memory segment to write `ClockErrorBound` data to.
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
    pub fn new(
        path: &Path,
        minimum_version: ClockErrorBoundLayoutVersion,
        current_version: ClockErrorBoundLayoutVersion,
    ) -> std::io::Result<ShmWriter> {
        // Determine the size of the segment.
        let segsize = ShmWriter::segment_size(current_version);

        // Use the ShmReader to assert the state of the segment. If the segment does not exist or
        // cannot be read correctly, wipe it clean. Note that there is a strong assumption here
        // that there is only one writer running on the system and writing to `path`. Consequently,
        // it is safe to wipe clean and then update. No-one will attempt to write to the segment
        // even if this process is scheduled out.
        if ShmWriter::is_usable_segment(path).is_err() {
            // Note that wiping the file sets the version to 0, which is used to indicate the
            // readers that the memory segment is not usable yet.
            ShmWriter::wipe(path, segsize)?;
        }

        // Memory map the file.
        let addr = ShmWriter::mmap_segment_at(path, segsize)?;

        // Obtain raw pointers to relevant members in the memory map segment and create a new
        // writer.
        // SAFETY: the segment has been validated to be usable, so these pointers are in-bounds.
        // The payload pointer casts to the concrete `#[repr(C)]` payload type selected by
        // `current_version` (never the layout-unstable `ClockErrorBound` enum). Its byte offset,
        // immediately after the 16-byte `ShmHeader`, is unchanged and correctly aligned for the
        // payload struct.
        let (generation, version, ceb) = unsafe {
            let cursor: *mut u8 = addr.cast();
            let generation = ptr::addr_of_mut!((*cursor.cast::<ShmHeader>()).generation);
            let version = ptr::addr_of_mut!((*cursor.cast::<ShmHeader>()).version);
            let ceb = match current_version {
                ClockErrorBoundLayoutVersion::V2 => {
                    LayoutPtr::V2(addr.add(size_of::<ShmHeader>()).cast::<ClockErrorBoundV2>())
                }
                ClockErrorBoundLayoutVersion::V3 => {
                    LayoutPtr::V3(addr.add(size_of::<ShmHeader>()).cast::<ClockErrorBoundV3>())
                }
            };
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
        // - If the segment was wiped clean, this defines the memory layout. It is still not
        //   useable by readers, until the next `update()` is successful.
        // - If the segment existed and was valid, the version is over-written. The minimum_version
        //   SHOULD NOT change when writing to a given path.
        // - It is possible for the current version to increment, if the daemon has been upgraded
        //   and the new version is backward compatible with the miminum one.
        let (min_ver, cur_ver): (u16, u16) = match current_version {
            // FIXME: the version 2 of the layout does not support minimum_version
            // interpretation of the u16 field. Set that to zero deliberately until the
            // implementation changes.
            ClockErrorBoundLayoutVersion::V2 => (0, current_version.into()),
            ClockErrorBoundLayoutVersion::V3 => (minimum_version.into(), current_version.into()),
        };
        let version_value = (min_ver << 8) | (cur_ver & 0x00ff);

        // SAFETY: segment has been validated to be usable, can use pointers.
        unsafe {
            let version = &*writer.version;
            version.store(version_value, atomic::Ordering::Relaxed);
        }

        Ok(writer)
    }

    /// Check whether the memory segment already exist and is usable.
    ///
    /// The segment is usable if it can be opened at `path` and it can be read by a `ShmReader`.
    fn is_usable_segment(path: &Path) -> Result<(), ShmError> {
        let path_cstring = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            ShmError::SegmentNotInitialized(format!(
                "SHM segment path is not a valid C string [{}]",
                path.to_string_lossy()
            ))
        })?;

        match ShmReader::new_with_max_version_unchecked(path_cstring.as_c_str()) {
            Ok((_reader, max_version)) => {
                if max_version == CLOCKBOUND_SHM_LATEST_VERSION {
                    Ok(())
                } else {
                    Err(ShmError::SegmentVersionNotSupported(format!(
                        "Existing SHM segment maximum version ({max_version}) does not match daemon version ({CLOCKBOUND_SHM_LATEST_VERSION})"
                    )))
                }
            }
            Err(err) => Err(err),
        }
    }

    /// Return a segment size which is large enough to store everything we need.
    fn segment_size(version: ClockErrorBoundLayoutVersion) -> usize {
        // Need to hold the header and the bound on clock error data.
        let size = size_of::<ShmHeader>()
            + match version {
                ClockErrorBoundLayoutVersion::V2 => size_of::<ClockErrorBoundV2>(),
                ClockErrorBoundLayoutVersion::V3 => size_of::<ClockErrorBoundV3>(),
            };

        // Round up to have 64 bit alignment. Not absolutely required but convenient. Currently,
        // the size of the data shared is almost two order of magnitude smaller than the minimum
        // system page size (4096), so taking a quick shortcut and ignoring paging alignment
        // questions for now.
        if size.is_multiple_of(8) {
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
                    return Err(Error::other("Failed to extract parent dir name"));
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
                return Err(std::io::Error::other(format!(
                    "Failed to convert segment size {segsize:?} into u32 {e:?}"
                )));
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
            return Err(std::io::Error::other(format!(
                "SHM Writer implementation error: wrote {pos:?} bytes but segsize is {size:?} bytes"
            )));
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
        // SAFETY: pointers to fields in the memory segment have been validated on init. The
        // payload pointer is valid for and aligned to the concrete `#[repr(C)]` payload struct,
        // and only the active layout variant is ever written.
        unsafe {
            // Start by reading the generation value stored in the memory segment.
            let generation = &*self.generation;
            let g = generation.load(atomic::Ordering::Acquire);

            // Mark the beginning of the update into the memory segment.
            // The producer process may have error'ed or died in the middle of a previous update
            // and left things hanging with an odd generation number. Being a bit fancy, this is
            // our data anti-entropy protection, and make sure we enter the updating section with
            // an odd number.
            let g = if g & 0x0001 == 0 {
                // This should be the most common case
                g.wrapping_add(1)
            } else {
                g
            };
            generation.store(g, atomic::Ordering::Release);

            match (&self.ceb, ceb) {
                (LayoutPtr::V2(p), ClockErrorBound::V2(v)) => ptr::write(*p, *v),
                (LayoutPtr::V3(p), ClockErrorBound::V3(v)) => ptr::write(*p, *v),
                // The LayoutPtr variant is fixed at construction from the segment's layout
                // version; a mismatch here means the caller handed us a different-version
                // ClockErrorBound than the segment was created for, which is a programming
                // error, never a runtime condition.
                _ => unreachable!(
                    "ShmWriter layout/version mismatch: segment pointer variant does not match the ClockErrorBound being written"
                ),
            }

            // Mark the end of the update into the memory segment by incrementing the generation
            // number. Note that we skip writing a generation equals to 0 when the counter rolls
            // over. This helps avoid a scenario that would otherwise lead to a bad outcome:
            //   1. the reader reads the generation number which happens to be 0 (just rolled over).
            //   2. the writer resets the memory segment, version and generation are at 0
            //   3. the reader finishes its read, generation is still at 0, but the read is dirty.
            //   4. the writer updates and set version, but it is too late by now.
            //
            // Skipping over a generation equals to 0 avoid this problem.
            let mut g = g.wrapping_add(1);
            if g == 0 {
                g = 2;
            }

            generation.store(g, atomic::Ordering::Release);
        }
    }
}

impl Drop for ShmWriter {
    /// Unmap the memory segment
    ///
    /// TODO: revisit to see if this can be refactored into the `MmapGuard` logic implemented on the
    /// `ShmReader`.
    fn drop(&mut self) {
        unsafe {
            nix::sys::mman::munmap(self.addr, self.segsize).expect("munmap");
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use byteorder::{NativeEndian, ReadBytesExt};
    use bytes::Buf;
    use nix::sys::time::TimeSpec;
    use std::fs::OpenOptions;
    use std::io::Read;
    use std::path::Path;
    /// We make use of tempfile::NamedTempFile to ensure that
    /// local files that are created during a test get removed
    /// afterwards.
    use tempfile::NamedTempFile;

    use crate::shm::shm_header::SHM_MAGIC;
    use crate::shm::{
        ClockErrorBound, ClockErrorBoundGeneric, ClockErrorBoundLayoutVersion, ClockStatus,
    };

    macro_rules! clockerrorbound {
        ($version:expr) => {
            ClockErrorBoundGeneric::builder()
                .as_of(TimeSpec::new(1, 2))
                .void_after(TimeSpec::new(3, 4))
                .bound_nsec(123)
                .disruption_marker(10)
                .max_drift_ppb(100)
                .clock_status(ClockStatus::Synchronized)
                .clock_disruption_support_enabled(true)
                .build($version)
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
    fn writer_create_new_if_not_exist() {
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        remove_path_if_exists(clockbound_shm_path);

        // Create and wipe the memory segment
        let ceb = clockerrorbound!(ClockErrorBoundLayoutVersion::V3);
        let mut writer = ShmWriter::new(
            Path::new(clockbound_shm_path),
            ClockErrorBoundLayoutVersion::V3,
            ClockErrorBoundLayoutVersion::V3,
        )
        .expect("Failed to create a writer");
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
    fn writer_wipe_clean_on_new() {
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
        let ceb = clockerrorbound!(ClockErrorBoundLayoutVersion::V3);
        let mut writer = ShmWriter::new(
            Path::new(clockbound_shm_path),
            ClockErrorBoundLayoutVersion::V3,
            ClockErrorBoundLayoutVersion::V3,
        )
        .expect("Failed to create a writer");
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
    fn writer_update_existing() {
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        remove_path_if_exists(clockbound_shm_path);

        // Create a clean memory segment
        let ceb = clockerrorbound!(ClockErrorBoundLayoutVersion::V2);
        let mut writer = ShmWriter::new(
            Path::new(clockbound_shm_path),
            ClockErrorBoundLayoutVersion::V2,
            ClockErrorBoundLayoutVersion::V2,
        )
        .expect("Failed to create a writer");

        // Push two updates to the shared memory segment, the generation moves from 0, to 2, to 4
        writer.write(&ceb);
        writer.write(&ceb);

        // Check what the writer says
        let generation = unsafe { &*writer.generation };
        let g = generation.load(atomic::Ordering::Acquire);
        std::mem::drop(writer);
        assert_eq!(g, 4);

        // Raw validation in the file
        // A bit brittle, would be more robust not to hardcode the seek to the generation field
        let mut file = std::fs::File::open(clockbound_shm_path).expect("create file failed");
        file.seek(std::io::SeekFrom::Start(14))
            .expect("Failed to seek to generation offset");
        let g = file
            .read_u16::<NativeEndian>()
            .expect("Failed to read generation from file");
        assert_eq!(g, 4);
    }

    /// Flush the writer's mapped segment to its backing file and read the whole file back.
    ///
    /// Uses `msync(MS_SYNC)` so the mapped bytes are guaranteed to be on the backing file before
    /// we re-open and read it. Reading through a fresh `File` (rather than the mapping) is what
    /// lets us confirm the wire-protocol matches
    fn msync_and_read(writer: &ShmWriter, path: &str) -> Vec<u8> {
        // SAFETY: `addr`/`segsize` describe the live mapping owned by `writer`, and MS_SYNC on a
        // shared mapping is always safe. Pointers are valid for the lifetime of the borrow.
        unsafe {
            nix::sys::mman::msync(
                writer.addr,
                writer.segsize,
                nix::sys::mman::MsFlags::MS_SYNC,
            )
            .expect("msync failed");
        }

        let mut buf = Vec::new();
        std::fs::File::open(path)
            .expect("open backing file failed")
            .read_to_end(&mut buf)
            .expect("read backing file failed");
        buf
    }

    /// Assert the V2 segment written by `ShmWriter` matches the Protocol Version 2 layout
    /// byte-for-byte, field by field, in the order documented in `docs/protocol.md`.
    #[test]
    fn writer_v2_matches_protocol_layout() {
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        remove_path_if_exists(clockbound_shm_path);

        // Deterministic, bitwise-unique values for every field so a shifted layout cannot
        // coincidentally match.
        let as_of = TimeSpec::new(0x1111_1111, 0x2222_2222);
        let void_after = TimeSpec::new(0x3333_3333, 0x4444_4444);
        let bound_nsec: i64 = 0x5555_5555_5555_5555;
        let disruption_marker: u64 = 0x6666_6666_6666_6666;
        let max_drift_ppb: u32 = 0x0777_7777; // < 1_000_000_000 so it is not rejected
        let clock_status = ClockStatus::FreeRunning; // == 2, distinct from the padding bytes

        let ceb = ClockErrorBoundGeneric::builder()
            .as_of(as_of)
            .void_after(void_after)
            .bound_nsec(bound_nsec)
            .disruption_marker(disruption_marker)
            .max_drift_ppb(max_drift_ppb)
            .clock_status(clock_status)
            .clock_disruption_support_enabled(true)
            .build(ClockErrorBoundLayoutVersion::V2);
        assert!(
            matches!(ceb, ClockErrorBound::V2(_)),
            "expected a V2 layout"
        );

        let mut writer = ShmWriter::new(
            Path::new(clockbound_shm_path),
            ClockErrorBoundLayoutVersion::V2,
            ClockErrorBoundLayoutVersion::V2,
        )
        .expect("Failed to create a writer");
        writer.write(&ceb);

        let raw = msync_and_read(&writer, clockbound_shm_path);
        // `&[u8]` implements `bytes::Buf`; each read advances the cursor from the front.
        let mut buf: &[u8] = &raw;

        // --- Header (ShmHeader, repr(C, align(8)), 16 bytes) ---------------------------------
        // Magic Number (u64): two u32 words, native endian, must be untouched.
        assert_eq!(buf.get_u32_ne(), SHM_MAGIC[0]);
        assert_eq!(buf.get_u32_ne(), SHM_MAGIC[1]);
        // Segment Size (u32): matches what the writer sized the segment to.
        assert_eq!(buf.get_u32_ne() as usize, writer.segsize);
        // Min Version | Max Version (u8 | u8) packed into a u16 field. The low byte is the max
        // version, the high byte the min version. V2 sets min=0, max=2 (see ShmWriter::new).
        assert_eq!(buf.get_u8(), 2);
        assert_eq!(buf.get_u8(), 0);
        // Generation (u16): a single successful write leaves an even, non-zero generation.
        assert_eq!(buf.get_u16_ne(), 2);

        // --- Data region: Protocol Version 2 layout, immediately follows the 16-byte header --
        // As-Of Timestamp (i64, i64)
        assert_eq!(buf.get_i64_ne(), as_of.tv_sec());
        assert_eq!(buf.get_i64_ne(), as_of.tv_nsec());
        // Void-After Timestamp (i64, i64)
        assert_eq!(buf.get_i64_ne(), void_after.tv_sec());
        assert_eq!(buf.get_i64_ne(), void_after.tv_nsec());
        // Bound (i64)
        assert_eq!(buf.get_i64_ne(), bound_nsec);
        // Disruption Marker (u64)
        assert_eq!(buf.get_u64_ne(), disruption_marker);
        // Max Drift (u32)
        assert_eq!(buf.get_u32_ne(), max_drift_ppb);
        // Clock Status (i32)
        assert_eq!(buf.get_i32_ne(), clock_status as i32);
        // Clock Disruption Support (u8)
        assert_eq!(buf.get_u8(), 1);
    }

    /// Assert the V3 segment written by `ShmWriter` matches the Protocol Version 3 layout
    /// byte-for-byte, field by field, in the order documented in `docs/protocol.md`.
    #[test]
    fn writer_v3_matches_protocol_layout() {
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        remove_path_if_exists(clockbound_shm_path);

        // Deterministic, bitwise-unique values for every field.
        let as_of_tsc: u64 = 0x0102_0304_0506_0708;
        let as_of = TimeSpec::new(0x1111_1111, 0x2222_2222);
        let void_after = TimeSpec::new(0x3333_3333, 0x4444_4444);
        // Distinct sub-second periods (both < 1s, so PeriodFrac does not panic) that produce
        // different frac/shift encodings.
        let period: f64 = 2.5e-9; // ~400 MHz oscillator
        let period_err: f64 = 1e-11;
        let bound_nsec: i64 = 0x5555_5555_5555_5555;
        let disruption_marker: u64 = 0x6666_6666_6666_6666;
        let max_drift_ppb: u32 = 0x0777_7777; // < 1_000_000_000
        let clock_status = ClockStatus::Disrupted; // == 3

        let ceb = ClockErrorBoundGeneric::builder()
            .as_of_tsc(as_of_tsc)
            .as_of(as_of)
            .void_after(void_after)
            .period(period)
            .period_err(period_err)
            .bound_nsec(bound_nsec)
            .disruption_marker(disruption_marker)
            .max_drift_ppb(max_drift_ppb)
            .clock_status(clock_status)
            .clock_disruption_support_enabled(true)
            .build(ClockErrorBoundLayoutVersion::V3);
        // Pull the expected scaled-fraction encodings straight from the payload the writer should
        // be serializing. `t_writer` is a submodule of `shm`, so the private fields are visible.
        let ClockErrorBound::V3(expected) = ceb else {
            panic!("expected a V3 layout");
        };

        let mut writer = ShmWriter::new(
            Path::new(clockbound_shm_path),
            ClockErrorBoundLayoutVersion::V3,
            ClockErrorBoundLayoutVersion::V3,
        )
        .expect("Failed to create a writer");
        writer.write(&ceb);

        let raw = msync_and_read(&writer, clockbound_shm_path);
        // `&[u8]` implements `bytes::Buf`; each read advances the cursor from the front.
        let mut buf: &[u8] = &raw;

        // --- Header (ShmHeader, repr(C, align(8)), 16 bytes) ---------------------------------
        assert_eq!(buf.get_u32_ne(), SHM_MAGIC[0]);
        assert_eq!(buf.get_u32_ne(), SHM_MAGIC[1]);
        assert_eq!(buf.get_u32_ne() as usize, writer.segsize);
        // V3 sets both min and max version to 3 (see ShmWriter::new).
        assert_eq!(buf.get_u8(), 3);
        assert_eq!(buf.get_u8(), 3);
        assert_eq!(buf.get_u16_ne(), 2);

        // --- Data region: Protocol Version 3 layout, immediately follows the 16-byte header --
        // As-Of TSC (u64)
        assert_eq!(buf.get_u64_ne(), as_of_tsc);
        // As-Of Timestamp (i64, i64)
        assert_eq!(buf.get_i64_ne(), as_of.tv_sec());
        assert_eq!(buf.get_i64_ne(), as_of.tv_nsec());
        // Void-After Timestamp (i64, i64)
        assert_eq!(buf.get_i64_ne(), void_after.tv_sec());
        assert_eq!(buf.get_i64_ne(), void_after.tv_nsec());
        // Period (u64)
        assert_eq!(buf.get_u64_ne(), expected.period_frac);
        // Period Error (u64)
        assert_eq!(buf.get_u64_ne(), expected.period_err_frac,);
        // Bound (i64)
        assert_eq!(buf.get_i64_ne(), bound_nsec);
        // Disruption Marker (u64)
        assert_eq!(buf.get_u64_ne(), disruption_marker);
        // Max Drift (u32)
        assert_eq!(buf.get_u32_ne(), max_drift_ppb);
        // Clock Status (i32)
        assert_eq!(buf.get_i32_ne(), clock_status as i32);
        // Clock Disruption Support (u8)
        assert_eq!(buf.get_u8(), 1);
        // Period Shift (u8), Period Error Shift (u8)
        assert_eq!(buf.get_u8(), expected.period_shift);
        assert_eq!(buf.get_u8(), expected.period_err_shift);
    }
}
