pub const HZ: u64 = crate::bindings::HZ as u64;

/// Returns current jiffies (as 64-bit number).
#[inline(always)]
pub fn jiffies() -> u64 {
    unsafe {
        // SAFETY:
        // jiffies_64 is correct variable that can be safely read.
        // It's also safe to read this static mut variable in Linux kernel.
        core::ptr::read_volatile(&crate::bindings::jiffies_64 as *const _)
    }
}
