//! Light weight representation of the vmclock shared memory structure
use clock_bound::vmclock::shm::VMClockClockStatus;
use std::fs::File;
use std::io::Write;
use std::ptr::from_ref;

pub fn write_vmclock_content(file: &mut File, vmclock_content: &VMClockContent) {
    // Convert the VMClockShmBody struct into a slice so we can write it all out, fairly magic.
    // Definitely needs the #[repr(C)] layout.
    let slice = unsafe {
        ::core::slice::from_raw_parts(
            from_ref::<VMClockContent>(vmclock_content).cast::<u8>(),
            ::core::mem::size_of::<VMClockContent>(),
        )
    };

    file.write_all(slice).expect("Write failed VMClockContent");
    file.sync_all().expect("Sync to disk failed");
}

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct VMClockContent {
    magic: u32,
    size: u32,
    version: u16,
    counter_id: u8,
    time_type: u8,
    pub seq_count: u32, // Used to indicate a clock disruption event
    pub disruption_marker: u64,
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

impl Default for VMClockContent {
    fn default() -> Self {
        VMClockContent {
            magic: 0x4B4C_4356,
            size: 104_u32,
            version: 1_u16,
            counter_id: 1_u8,
            time_type: 0_u8,
            seq_count: 10_u32,
            disruption_marker: 888_888_u64,
            flags: 0_u64,
            _padding: [0x00, 0x00],
            clock_status: VMClockClockStatus::Synchronized,
            leap_second_smearing_hint: 0_u8,
            tai_offset_sec: 0_i16,
            leap_indicator: 0_u8,
            counter_period_shift: 0_u8,
            counter_value: 123_456_u64,
            counter_period_frac_sec: 0_u64,
            counter_period_esterror_rate_frac_sec: 0_u64,
            counter_period_maxerror_rate_frac_sec: 0_u64,
            time_sec: 0_u64,
            time_frac_sec: 0_u64,
            time_esterror_nanosec: 0_u64,
            time_maxerror_nanosec: 0_u64,
        }
    }
}
