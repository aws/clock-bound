//! Module for reading TSC values.
#[cfg_attr(test, mockall::automock)]
pub trait ReadTsc {
    fn read_tsc(&self) -> u64;
}
pub struct ReadTscImpl;
impl ReadTsc for ReadTscImpl {
    fn read_tsc(&self) -> u64 {
        read_timestamp_counter()
    }
}

/// Reads the current value of the processor's time-stamp counter.
#[cfg(target_arch = "aarch64")]
pub fn read_timestamp_counter() -> u64 {
    // aarch64 documentation: https://developer.arm.com/documentation/ddi0601/2021-12/AArch64-Registers/CNTVCT-EL0--Counter-timer-Virtual-Count-register
    use std::arch::asm;

    let rv: u64;
    unsafe {
        asm!("mrs {}, cntvct_el0", out(reg) rv);
    }
    rv
}

#[cfg(target_arch = "x86_64")]
pub fn read_timestamp_counter() -> u64 {
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
    use core::arch::x86_64::_rdtsc;
    unsafe { _rdtsc() }
}
