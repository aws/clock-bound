//! VMClock Shared Memory
//!
//! This crate implements the low-level implementation to share VMClock data
//! over a shared memory segment. This crate is meant to be used by the C and Rust versions
//! of the ClockBound client library.

// TODO: prevent clippy from checking for dead code. The writer module is only re-exported publicly
// if the write feature is selected. There may be a better way to do that and re-enable the lint.
#![allow(dead_code)]

use std::mem::size_of;
use std::str::FromStr;
use std::sync::atomic;

use clock_bound_shm::{syserror, ShmError};
use tracing::{debug, error};

pub const VMCLOCK_SHM_DEFAULT_PATH: &str = "/dev/vmclock0";

/// The magic number that identifies a VMClock shared memory segment.
pub const VMCLOCK_SHM_MAGIC: u32 = 0x4B4C4356;

/// Header structure to the Shared Memory segment where the VMClock data is kept.
///
/// Most members are atomic types as they are subject to be updated by the Hypervisor.
#[repr(C)]
#[derive(Debug)]
pub struct VMClockShmHeader {
    /// Magic number to uniquely identify the content of the memory segment.
    pub magic: atomic::AtomicU32,

    /// Size of the memory segment.
    pub size: atomic::AtomicU32,

    /// Version identifying the layout of data written to the shared memory segment.
    pub version: atomic::AtomicU16,

    /// Counter ID.
    pub counter_id: atomic::AtomicU8,

    /// Time type.
    ///
    /// Possible values are:
    ///
    /// VMCLOCK_TIME_UTC                        0   // Since 1970-01-01 00:00:00z
    /// VMCLOCK_TIME_TAI                        1   // Since 1970-01-01 00:00:00z
    /// VMCLOCK_TIME_MONOTONIC                  2   // Since undefined epoch
    /// VMCLOCK_TIME_INVALID_SMEARED            3   // Not supported
    /// VMCLOCK_TIME_INVALID_MAYBE_SMEARED      4   // Not supported
    ///
    pub time_type: atomic::AtomicU8,

    // Sequence count that is a generation number incremented by the
    // VMClock on every update of the shared memory segment.
    //
    // Odd numbers mean that an update is in progress.
    pub seq_count: atomic::AtomicU32,
}

impl VMClockShmHeader {
    /// Initialize a VMClockShmHeader from a vector of bytes.
    ///
    /// It is assumed that the vecxtor has already been validated to have enough bytes to hold.
    pub fn read(vector: &Vec<u8>) -> Result<Self, ShmError> {
        if vector.len() < size_of::<VMClockShmHeader>() {
            return syserror!("Insufficient bytes to create a VMClockShmHeader.");
        }

        let slice = vector.as_slice();

        let magic = u32::from_le_bytes(slice[0..4].try_into().unwrap());
        let size = u32::from_le_bytes(slice[4..8].try_into().unwrap());
        let version = u16::from_le_bytes(slice[8..10].try_into().unwrap());
        let counter_id = u8::from_le_bytes(slice[10..11].try_into().unwrap());
        let time_type = u8::from_le_bytes(slice[11..12].try_into().unwrap());
        let seq_count = u32::from_le_bytes(slice[12..16].try_into().unwrap());

        let header = VMClockShmHeader {
            magic: atomic::AtomicU32::new(magic),
            size: atomic::AtomicU32::new(size),
            version: atomic::AtomicU16::new(version),
            counter_id: atomic::AtomicU8::new(counter_id),
            time_type: atomic::AtomicU8::new(time_type),
            seq_count: atomic::AtomicU32::new(seq_count),
        };

        header.is_valid()?;
        Ok(header)
    }

    /// Check whether the magic number matches the expected one.
    fn matches_magic(&self) -> bool {
        let magic = self.magic.load(atomic::Ordering::Relaxed);
        debug!("VMClockShmHeader has magic: {:?}", magic);
        magic == VMCLOCK_SHM_MAGIC
    }

    /// Check whether the header is marked with a valid version
    fn has_valid_version(&self) -> bool {
        let version = self.version.load(atomic::Ordering::Relaxed);
        debug!("VMClockShmHeader has version: {:?}", version);
        version > 0
    }

    /// Check whether the header is complete
    fn is_well_formed(&self) -> bool {
        let size = self.size.load(atomic::Ordering::Relaxed);
        debug!("VMClockShmHeader has size: {:?}", size);
        size as usize >= size_of::<Self>()
    }

    /// Check whether a VMClockShmHeader is valid
    fn is_valid(&self) -> Result<(), ShmError> {
        if !self.matches_magic() {
            error!("VMClockShmHeader does not have a matching magic number.");
            return Err(ShmError::SegmentMalformed);
        }

        if !self.has_valid_version() {
            error!("VMClockShmHeader does not have a valid version number.");
            return Err(ShmError::SegmentNotInitialized);
        }

        if !self.is_well_formed() {
            error!("VMClockShmHeader is not well formed.");
            return Err(ShmError::SegmentMalformed);
        }
        Ok(())
    }
}

/// Definition of mutually exclusive clock status exposed to the reader.
#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum VMClockClockStatus {
    /// The status of the clock is unknown.
    Unknown = 0,

    /// The clock is being initialized.
    Initializing = 1,

    /// The clock is kept accurate by the synchronization daemon.
    Synchronized = 2,

    /// The clock is free running and not updated by the synchronization daemon.
    FreeRunning = 3,

    /// The clock is unreliable and should not be trusted.
    /// The reason is unspecified, but it could be due to the hardware counter being broken.
    Unreliable = 4,
}

/// Custom struct used for indicating a parsing error when parsing a
/// VMClockClockStatus from str.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ParseError;

impl FromStr for VMClockClockStatus {
    type Err = ParseError;
    fn from_str(input: &str) -> Result<VMClockClockStatus, Self::Err> {
        match input {
            "Unknown" => Ok(VMClockClockStatus::Unknown),
            "Initializing" => Ok(VMClockClockStatus::Initializing),
            "Synchronized" => Ok(VMClockClockStatus::Synchronized),
            "FreeRunning" => Ok(VMClockClockStatus::FreeRunning),
            "Unreliable" => Ok(VMClockClockStatus::Unreliable),
            _ => Err(ParseError),
        }
    }
}

/// Structure that holds the VMClock data captured at a specific point in time.
///
/// The structure is shared across the Shared Memory segment and has a C representation to enforce
/// this specific layout.
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct VMClockShmBody {
    /// Disruption Marker.
    ///
    /// This value is incremented (by an unspecified delta) each time the clock is disrupted.
    /// This value is specific to a particular VM/EC2 instance.
    pub disruption_marker: u64,

    /// Flags.
    ///
    /// Bit flags representing the following:
    ///
    /// Bit (1 << 0): VMCLOCK_FLAG_TAI_OFFSET_VALID: Indicates that the tai_offset_sec field is valid.
    ///
    /// The below bits are optionally used to notify guests of pending
    /// maintenance events. A guest which provides latency-sensitive
    /// services may wish to remove itself from service if an event is coming up.
    /// Two flags indicate the approximate imminence of the event.
    ///
    /// Bit (1 << 1): VMCLOCK_FLAG_DISRUPTION_SOON: About a day.
    /// Bit (1 << 2): VMCLOCK_FLAG_DISRUPTION_IMMINENT: About an hour.
    /// Bit (1 << 3): VMCLOCK_FLAG_PERIOD_ESTERROR_VALID
    /// Bit (1 << 4): VMCLOCK_FLAG_PERIOD_MAXERROR_VALID
    /// Bit (1 << 5): VMCLOCK_FLAG_TIME_ESTERROR_VALID
    /// Bit (1 << 6): VMCLOCK_FLAG_TIME_MAXERROR_VALID
    ///
    /// The below bit is the MONOTONIC flag.
    /// If the MONOTONIC flag is set then (other than leap seconds) it is
    /// guaranteed that the time calculated according this structure at
    /// any given moment shall never appear to be later than the time
    /// calculated via the structure at any *later* moment.
    ///
    /// In particular, a timestamp based on a counter reading taken
    /// immediately after setting the low bit of seq_count (and the
    /// associated memory barrier), using the previously-valid time and
    /// period fields, shall never be later than a timestamp based on
    /// a counter reading taken immediately before *clearing* the low
    /// bit again after the update, using the about-to-be-valid fields.
    ///
    /// Bit (1 << 7): VMCLOCK_FLAG_TIME_MONOTONIC
    ///
    pub flags: u64,

    /// Padding.
    pub _padding: [u8; 2],

    /// Clock Status.
    ///
    /// The clock status indicates whether the clock is synchronized,
    /// free-running, etc.
    pub clock_status: VMClockClockStatus,

    // Leap Second smearing hint.
    //
    /// The time exposed through this device is never smeared. This field
    /// corresponds to the 'subtype' field in virtio-rtc, which indicates
    /// the smearing method. However in this case it provides a *hint* to
    /// the guest operating system, such that *if* the guest OS wants to
    /// provide its users with an alternative clock which does not follow
    /// UTC, it may do so in a fashion consistent with the other systems
    /// in the nearby environment.
    ///
    /// Possible values are:
    ///
    /// VMCLOCK_SMEARING_STRICT:      0
    /// VMCLOCK_SMEARING_NOON_LINEAR: 1
    /// VMCLOCK_SMEARING_UTC_SLS:     2
    ///
    pub leap_second_smearing_hint: u8,

    /// Offset between TAI and UTC, in seconds.
    pub tai_offset_sec: i16,

    /// Leap indicator.
    ///
    /// This field is based on the the VIRTIO_RTC_LEAP_xxx values as
    /// defined in the current draft of virtio-rtc, but since smearing
    /// cannot be used with the shared memory device, some values are
    /// not used.
    ///
    /// The _POST_POS and _POST_NEG values allow the guest to perform
    /// its own smearing during the day or so after a leap second when
    /// such smearing may need to continue being applied for a leap
    /// second which is now theoretically "historical".
    ///
    /// Possible values are:
    /// VMCLOCK_LEAP_NONE       0x00  // No known nearby leap second
    /// VMCLOCK_LEAP_PRE_POS    0x01  // Positive leap second at EOM
    /// VMCLOCK_LEAP_PRE_NEG    0x02  // Negative leap second at EOM
    /// VMCLOCK_LEAP_POS        0x03  // Set during 23:59:60 second
    /// VMCLOCK_LEAP_POST_POS   0x04
    /// VMCLOCK_LEAP_POST_NEG   0x05
    ///
    pub leap_indicator: u8,

    /// Counter period shift.
    ///
    /// Bit shift for the counter_period_frac_sec and its error rate.
    pub counter_period_shift: u8,

    /// Counter value.
    pub counter_value: u64,

    /// Counter period.
    ///
    /// This is the estimated period of the counter, in binary fractional seconds.
    /// The unit of this field is: 1 / (2 ^ (64 + counter_period_shift)) of a second.
    pub counter_period_frac_sec: u64,

    /// Counter period estimated error rate.
    ///
    /// This is the estimated error rate of the counter period, in binary fractional seconds per second.
    /// The unit of this field is: 1 / (2 ^ (64 + counter_period_shift)) of a second per second.
    pub counter_period_esterror_rate_frac_sec: u64,

    /// Counter period maximum error rate.
    ///
    /// This is the maximum error rate of the counter period, in binary fractional seconds per second.
    /// The unit of this field is: 1 / (2 ^ (64 + counter_period_shift)) of a second per second.
    pub counter_period_maxerror_rate_frac_sec: u64,

    /// Time according to the time_type field.

    /// Time: Seconds since time_type epoch.
    pub time_sec: u64,

    /// Time: Fractional seconds, in units of 1 / (2 ^ 64) of a second.
    pub time_frac_sec: u64,

    /// Time: Estimated error.
    pub time_esterror_nanosec: u64,

    /// Time: Maximum error.
    pub time_maxerror_nanosec: u64,
}

impl Default for VMClockShmBody {
    /// Get a default VMClockShmBody struct
    /// Equivalent to zero'ing this bit of memory
    fn default() -> Self {
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
    }
}

#[cfg(test)]
mod t_vmclock_shm_header {
    use super::*;
    use byteorder::{LittleEndian, WriteBytesExt};
    use std::fs::{File, OpenOptions};
    use std::io::Read;
    use std::path::Path;
    /// We make use of tempfile::NamedTempFile to ensure that
    /// local files that are created during a test get removed
    /// afterwards.
    use tempfile::NamedTempFile;

    macro_rules! write_vmclock_shm_header {
        ($file:ident,
         $magic:literal,
         $size:literal,
         $version:literal,
         $counter_id:literal,
         $time_type:literal,
         $seq_count:literal) => {
            $file
                .write_u32::<LittleEndian>($magic)
                .expect("Write failed magic");
            $file
                .write_u32::<LittleEndian>($size)
                .expect("Write failed size");
            $file
                .write_u16::<LittleEndian>($version)
                .expect("Write failed version");
            $file
                .write_u8($counter_id)
                .expect("Write failed counter_id");
            $file.write_u8($time_type).expect("Write failed time_type");
            $file
                .write_u32::<LittleEndian>($seq_count)
                .expect("Write failed seq_count");
            $file.sync_all().expect("Sync to disk failed");
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

    /// Assert that a file containing a valid header produces a valid VMClockShmHeader
    #[test]
    fn test_header_valid() {
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        let mut vmclock_shm_file = OpenOptions::new()
            .write(true)
            .open(vmclock_shm_path)
            .expect("open vmclock file failed");
        write_vmclock_shm_header!(
            vmclock_shm_file,
            0x4B4C4356,
            104_u32,
            1_u16,
            0_u8,
            0_u8,
            99_u32
        );

        let mut file = File::open(vmclock_shm_path).expect("failed to open file");
        let mut buffer = vec![];
        file.read_to_end(&mut buffer)
            .expect("failed to read to end of the file");
        let header = VMClockShmHeader::read(&buffer).expect("SHM Reader read");

        assert_eq!(header.magic.into_inner(), 0x4B4C4356);
        assert_eq!(header.size.into_inner(), 104_u32);
        assert_eq!(header.version.into_inner(), 1_u16);
        assert_eq!(header.counter_id.into_inner(), 0_u8);
        assert_eq!(header.time_type.into_inner(), 0_u8);
        assert_eq!(header.seq_count.into_inner(), 99_u32);
    }

    /// Assert that a file with a bad magic returns an error
    #[test]
    fn test_header_bad_magic() {
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        let mut vmclock_shm_file = OpenOptions::new()
            .write(true)
            .open(vmclock_shm_path)
            .expect("open vmclock file failed");
        // magic numbers are bogus
        write_vmclock_shm_header!(
            vmclock_shm_file,
            0xdeadbeef,
            16_u32,
            1_u16,
            0_u8,
            0_u8,
            99_u32
        );

        let mut file = File::open(vmclock_shm_path).expect("failed to open file");
        let mut buffer = vec![];
        file.read_to_end(&mut buffer)
            .expect("failed to read to end of the file");
        assert!(VMClockShmHeader::read(&buffer).is_err());
    }

    /// Assert that a file with a wrongly truncated header returns an error
    #[test]
    fn test_header_bad_size() {
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        let mut vmclock_shm_file = OpenOptions::new()
            .write(true)
            .open(vmclock_shm_path)
            .expect("open vmclock file failed");
        write_vmclock_shm_header!(
            vmclock_shm_file,
            0x4B4C4356,
            4_u32,
            1_u16,
            0_u8,
            0_u8,
            99_u32
        );

        let mut file = File::open(vmclock_shm_path).expect("failed to open file");
        let mut buffer = vec![];
        file.read_to_end(&mut buffer)
            .expect("failed to read to end of the file");
        assert!(VMClockShmHeader::read(&buffer).is_err());
    }

    /// Assert that a file with a version number of 0 returns an error
    #[test]
    fn test_header_bad_version() {
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        let mut vmclock_shm_file = OpenOptions::new()
            .write(true)
            .open(vmclock_shm_path)
            .expect("open vmclock file failed");
        // layout version is 0
        write_vmclock_shm_header!(
            vmclock_shm_file,
            0x4B4C4356,
            16_u32,
            0_u16,
            0_u8,
            0_u8,
            99_u32
        );

        let mut file = File::open(vmclock_shm_path).expect("failed to open file");
        let mut buffer = vec![];
        file.read_to_end(&mut buffer)
            .expect("failed to read to end of the file");
        assert!(VMClockShmHeader::read(&buffer).is_err());
    }
}
