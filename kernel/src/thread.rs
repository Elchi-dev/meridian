//! Kernel threads, the parts that need no processor: where each thread's
//! stack lies, and what a new thread's stack holds before it first runs.

/// How many threads there can be at once, the first one and every core's
/// idle thread included.
pub const THREADS: usize = 256;

/// Where the threads' stacks lie. Each gets 64 KiB of address space, of
/// which only the top 32 KiB are mapped, so a stack that runs over faults
/// instead of writing into the next one.
pub const STACKS: u64 = 0xffff_fc80_0000_0000;

/// The pages of each thread's stack.
pub const STACK_PAGES: u64 = 8;

/// The top of the stack of thread `slot`.
#[must_use]
pub fn stack_top(slot: usize) -> u64 {
    STACKS + (slot as u64 + 1) * 2 * STACK_PAGES * 4096
}

/// The words a new thread's stack starts with, lowest first, in the order
/// a switch takes them back: its flags, the six registers a switch keeps,
/// with `entry` in r12 and `argument` in r13, and `start`, where the
/// switch returns to.
#[must_use]
pub fn first_frame(flags: u64, entry: u64, argument: u64, start: u64) -> [u64; 8] {
    [flags, 0, 0, argument, entry, 0, 0, start]
}

/// Where that frame goes below the stack's top `top`: low enough that the
/// stack is aligned to 16 bytes again when `start` calls the entry, as the
/// calling convention wants.
#[must_use]
pub fn first_frame_at(top: u64) -> u64 {
    top - 80
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_thread_stack_has_a_guard_below_and_meets_no_other() {
        assert_eq!(stack_top(0), 0xffff_fc80_0001_0000);
        assert_eq!(stack_top(1), 0xffff_fc80_0002_0000);
        for slot in 1..THREADS {
            assert_eq!(
                stack_top(slot) - stack_top(slot - 1),
                0x1_0000,
                "32 KiB of stack, 32 KiB of guard"
            );
        }
        assert_eq!(
            STACKS >> 39 & 0x1ff,
            505,
            "a slot of the root table of its own"
        );
        assert_eq!((stack_top(THREADS - 1) - 1) >> 39 & 0x1ff, 505);
        assert!(stack_top(THREADS - 1) <= crate::APIC_PAGE);
    }

    #[test]
    fn there_are_threads_left_when_every_core_idles() {
        // Every core keeps one thread as its idle one, and the first core
        // keeps the one that started the kernel too.
        let kept = crate::CORES + 1;
        assert!(
            THREADS - kept >= 128,
            "{} threads left on the most cores",
            THREADS - kept
        );
        assert!(
            u16::try_from(THREADS).is_ok(),
            "lib/sched numbers threads in 16 bits"
        );
    }

    #[test]
    fn a_new_thread_starts_with_its_entry_and_argument_ready() {
        let frame = first_frame(0x202, 0xffff_ffff_8000_1000, 42, 0xffff_ffff_8000_2000);
        assert_eq!(
            frame,
            [
                0x202,
                0,
                0,
                42,
                0xffff_ffff_8000_1000,
                0,
                0,
                0xffff_ffff_8000_2000
            ]
        );
    }

    #[test]
    fn the_stack_is_aligned_when_the_entry_is_called() {
        for slot in [0, 1, THREADS - 1] {
            let top = stack_top(slot);
            let at = first_frame_at(top);
            // The switch takes eight words, then start pushes its return
            // address with the call: the entry has to see 8 more than a
            // multiple of 16.
            let at_call = at + 8 * 8;
            assert_eq!(at_call % 16, 0, "slot {slot}");
            assert_eq!((at_call - 8) % 16, 8);
            assert!(at_call < top, "the frame stays below the top");
            assert!(top - at <= STACK_PAGES * 4096);
        }
    }
}
