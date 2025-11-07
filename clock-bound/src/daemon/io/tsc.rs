//! Module for reading TSC values.
#[cfg_attr(test, mockall::automock)]
pub trait ReadTsc {
    fn read_tsc(&self) -> u64;
}
pub struct ReadTscImpl;
impl ReadTsc for ReadTscImpl {
    fn read_tsc(&self) -> u64 {
        read_timestamp_counter_begin()
    }
}

/// Brackets time-stamp counter read with synchronization barrier instructions.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn read_timestamp_counter_end() -> u64 {
    // aarch64 documentation: https://developer.arm.com/documentation/ddi0601/2021-12/AArch64-Registers/CNTVCT-EL0--Counter-timer-Virtual-Count-register
    use std::arch::asm;

    let rv: u64;
    unsafe {
        asm!("isb; mrs {}, cntvct_el0; isb;", out(reg) rv);
    }
    rv
}

/// Brackets time-stamp counter read with synchronization barrier instructions.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn read_timestamp_counter_begin() -> u64 {
    // aarch64 documentation: https://developer.arm.com/documentation/ddi0601/2021-12/AArch64-Registers/CNTVCT-EL0--Counter-timer-Virtual-Count-register
    // instruction barrier documentation: https://developer.arm.com/documentation/100941/0101/Barriers
    use std::arch::asm;

    let rv: u64;
    unsafe {
        asm!("isb; mrs {}, cntvct_el0; isb;", out(reg) rv);
    }
    rv
}

/// Reads the current value of the processor's time-stamp counter.
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn read_timestamp_counter_begin() -> u64 {
    /*
    There are a number of options for getting tsc values on x86_64 cpus.
    We could get them from the registers ourselves leveraging assembly
    ```
    // From: https://oliveryang.net/2015/09/pitfalls-of-TSC-usage/
    static uint64_t rdtsc(void)
    {
        uint64_t var;
        uint32_t hi, lo;

        __asm volatile
            ("rdtsc" : "=a" (lo), "=d" (hi));

        var = ((uint64_t)hi << 32) | lo;
        return (var);
    }
    ```

    Or we can get them from the llvm libs.
    https://doc.rust-lang.org/beta/src/core/stdarch/crates/core_arch/src/x86/rdtsc.rs.html#55
    core::arch::x86_64::_rdtsc;
    {
        _rdtsc()
    }

    I've chosen to get the values from llvm because as I'm confident they are implemented correctly.
    */
    // Fencing is discussed in Vol 2B 4-550 of Intel architecture software development manual
    use core::arch::x86_64::{_mm_lfence, _rdtsc};
    let tsc;
    unsafe {
        _mm_lfence();
        tsc = _rdtsc();
        _mm_lfence();
    }
    tsc
}

/// Applies a synchronization barrier then reads the current value of the processor's time-stamp counter.
#[cfg(target_arch = "x86_64")]
#[inline]
pub fn read_timestamp_counter_end() -> u64 {
    use core::arch::x86_64::{__rdtscp, _mm_lfence};
    // Fencing is discussed in Vol 2B 4-552 of Intel architecture software development manual
    // `__rdtscp` writes the IA32_TSC_AUX value to `aux`. IA32_TSC_AUX is usually the cpu id, but
    // the meaning depends on the operating system. Currently, we do not use this value.
    let mut aux = 0u32;
    let tsc;
    unsafe {
        tsc = __rdtscp(&raw mut aux);
        _mm_lfence();
    }
    tsc
}
