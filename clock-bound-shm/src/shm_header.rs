// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use std::mem::{size_of, MaybeUninit};
use std::sync::atomic;

use crate::{syserror, ShmError};

/// The magic number that identifies a ClockErrorBound shared memory segment.
pub const SHM_MAGIC: [u32; 2] = [0x414D5A4E, 0x43420200];

/// Header structure to the Shared Memory segment where the ClockErrorBound data is kept.
///
/// Most members are atomic types as they are subject to be updated by the ClockBound daemon.
#[repr(C, align(8))]
#[derive(Debug)]
pub struct ShmHeader {
    /// Magic number to uniquely identify the content of the memory segment.
    pub magic: [u32; 2],

    // Size of the segment that has been written to shared memory by the ShmWriter.
    pub segsize: atomic::AtomicU32,

    // Version identifying the layout of data written to the shared memory segment.
    pub version: atomic::AtomicU16,

    // Generation number incremented by the writer on every update of the shared memory segment.
    pub generation: atomic::AtomicU16,
}

impl ShmHeader {
    /// Initialize a ShmHeader from a file descriptor
    ///
    /// Read the content of a file, ensures it is meant to contain ClockErrorBound data by
    /// validating the magic number and return a valid header.
    pub fn read(fdesc: i32) -> Result<Self, ShmError> {
        let mut header_buf: MaybeUninit<ShmHeader> = MaybeUninit::uninit();
        // SAFETY: `buf` points to `count` bytes of valid memory.
        match unsafe {
            libc::read(
                fdesc,
                header_buf.as_mut_ptr().cast(),
                size_of::<ShmHeader>(),
            )
        } {
            ret if ret < 0 => return syserror!("read SHM segment"),
            ret if (ret as usize) < size_of::<ShmHeader>() => {
                return Err(ShmError::SegmentNotInitialized)
            }
            _ => (),
        };

        // SAFETY: we've checked the above return value to ensure header_buf
        // has been completely initialized by the previous read.
        let header = unsafe { header_buf.assume_init() };
        header.is_valid()?;

        Ok(header)
    }

    /// Check whether the magic number matches the expected one.
    fn matches_magic(&self, magic: &[u32; 2]) -> bool {
        self.magic == *magic
    }

    /// Check whether the header is marked with a valid version
    fn has_valid_version(&self) -> bool {
        let version = self.version.load(atomic::Ordering::Relaxed);
        version > 0
    }

    /// Check whether the header is initialized
    fn is_initialized(&self) -> bool {
        let generation = self.generation.load(atomic::Ordering::Relaxed);
        generation > 0
    }

    /// Check whether the header is complete
    fn is_well_formed(&self) -> bool {
        let segsize = self.segsize.load(atomic::Ordering::Relaxed);
        segsize as usize >= size_of::<Self>()
    }

    /// Check whether a ShmHeader is valid
    fn is_valid(&self) -> Result<(), ShmError> {
        if !self.matches_magic(&SHM_MAGIC) {
            return Err(ShmError::SegmentNotInitialized);
        }

        if !self.has_valid_version() {
            return Err(ShmError::SegmentNotInitialized);
        }

        if !self.is_initialized() {
            return Err(ShmError::SegmentNotInitialized);
        }

        if !self.is_well_formed() {
            return Err(ShmError::SegmentMalformed);
        }
        Ok(())
    }
}

#[cfg(test)]
mod t_shm_header {
    /// This test module is full of side effects and create local files to test the ShmHeader
    /// functionality. Tests run concurrently, so each test creates its own dedicated file.
    /// For now, create files in `/tmp/` and no cleaning is done.
    ///
    /// TODO: investigate how to retrieve the target-dir that would work for both brazil package
    /// and "native" cargo ones to contain artefacts better.
    ///
    use super::*;
    use byteorder::{NativeEndian, WriteBytesExt};
    use std::ffi::CString;
    use std::fs::File;

    macro_rules! write_shm_header {
        ($file:ident,
         $magic_0:literal,
         $magic_1:literal,
         $segsize:literal,
         $version:literal,
         $generation:literal) => {
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
            $file.sync_all().expect("Sync to disk failed");
        };
    }

    /// Assert that a file containing a valid header produces a valid ShmHeader
    #[test]
    fn test_header_valid() {
        let path_shm = "/tmp/test_header_valid";
        let mut file = File::create(path_shm).expect("create file failed");
        write_shm_header!(file, 0x414D5A4E, 0x43420200, 16, 1, 99);

        let path = CString::new(path_shm).expect("CString failed");
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY) };

        let reader = ShmHeader::read(fd).expect("SHM Reader read");

        assert_eq!(reader.segsize.into_inner(), 16);
        assert_eq!(reader.version.into_inner(), 1);
        assert_eq!(reader.generation.into_inner(), 99);
    }

    /// Assert that a file with a bad magic returns an error
    #[test]
    fn test_header_bad_magic() {
        let path_shm = "/tmp/test_header_bad_magic";
        let mut file = File::create(path_shm).expect("create file failed");
        // magic numbers are bogus
        write_shm_header!(file, 0xdeadbeef, 0x0badcafe, 16, 1, 99);

        let path = CString::new(path_shm).expect("CString failed");
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY) };

        assert!(ShmHeader::read(fd).is_err());
    }

    /// Assert that a file with a wrongly truncated header returns an error
    #[test]
    fn test_header_bad_segsize() {
        let path_shm = "/tmp/test_header_bad_segsize";
        let mut file = File::create(path_shm).expect("create file failed");
        // segsize = 4 instead of 16
        write_shm_header!(file, 0x414D5A4E, 0x43420200, 4, 1, 99);

        let path = CString::new(path_shm).expect("CString failed");
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY) };

        assert!(ShmHeader::read(fd).is_err());
    }

    /// Assert that a file with a version number of 0 returns an error
    #[test]
    fn test_header_bad_version() {
        let path_shm = "/tmp/test_header_bad_version";
        let mut file = File::create(path_shm).expect("create file failed");
        // layout version is 0
        write_shm_header!(file, 0x414D5A4E, 0x43420200, 16, 0, 99);

        let path = CString::new(path_shm).expect("CString failed");
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY) };

        assert!(ShmHeader::read(fd).is_err());
    }

    /// Assert that a file with a generation number of 0 returns an error
    #[test]
    fn test_header_bad_generation() {
        let path_shm = "/tmp/test_header_bad_generation";
        let mut file = File::create(path_shm).expect("create file failed");
        // generation number is 0
        write_shm_header!(file, 0x414D5A4E, 0x43420200, 16, 1, 0);

        let path = CString::new(path_shm).expect("CString failed");
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY) };

        assert!(ShmHeader::read(fd).is_err());
    }
}
