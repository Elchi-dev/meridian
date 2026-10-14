//! The descriptor tables the processor reads: the global descriptor table
//! with its code, data and task state segments, the task state segment
//! itself, and the interrupt descriptor table (Intel SDM, volume 3,
//! sections 3.4.5, 7.2.3, 7.7 and 6.14).

/// Selectors, the byte offsets of the entries in [`gdt`].
pub const KERNEL_CODE: u16 = 0x08;
pub const KERNEL_DATA: u16 = 0x10;
pub const TASK_STATE: u16 = 0x18;

/// 64-bit code: present, ring 0, readable, long mode.
const CODE: u64 = 0x00af_9a00_0000_ffff;
/// Data: present, ring 0, writable.
const DATA: u64 = 0x00cf_9200_0000_ffff;

/// The global descriptor table: the null entry, kernel code, kernel data,
/// and the two entries of the task state segment at `tss`.
#[must_use]
pub fn gdt(tss: u64) -> [u64; 5] {
    // The limit's bits 16 to 19 would go to bits 48 to 51; a task state
    // smaller than 64 KiB leaves them zero.
    let limit = u64::from(TASK_STATE_SIZE) - 1;
    let low = limit | (tss & 0xff_ffff) << 16 | 0x89 << 40 | (tss >> 24 & 0xff) << 56;
    [0, CODE, DATA, low, tss >> 32]
}

/// The size of [`TaskState`] in bytes.
const TASK_STATE_SIZE: u16 = 104;

/// The task state segment. In long mode it only holds stacks: the ones
/// for entering ring 0 from user mode, and seven interrupt stacks that a
/// gate can ask for, so a fault still has a stack when its own is gone.
#[derive(Clone, Copy, Debug, Default)]
#[repr(C, packed(4))]
pub struct TaskState {
    reserved0: u32,
    pub privilege_stacks: [u64; 3],
    reserved1: u64,
    pub interrupt_stacks: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    /// Past the end of the segment, so no I/O ports are allowed from user
    /// mode.
    pub io_map: u16,
}

impl TaskState {
    #[must_use]
    pub const fn new() -> TaskState {
        TaskState {
            reserved0: 0,
            privilege_stacks: [0; 3],
            reserved1: 0,
            interrupt_stacks: [0; 7],
            reserved2: 0,
            reserved3: 0,
            io_map: TASK_STATE_SIZE,
        }
    }
}

/// One entry of the interrupt descriptor table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Gate {
    offset_low: u16,
    selector: u16,
    stack: u8,
    attributes: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl Gate {
    /// A gate that is not there: using it is a general protection fault.
    pub const MISSING: Gate = Gate {
        offset_low: 0,
        selector: 0,
        stack: 0,
        attributes: 0,
        offset_mid: 0,
        offset_high: 0,
        reserved: 0,
    };

    /// An interrupt gate to `handler` in kernel code, which turns
    /// interrupts off on the way in. `stack` picks one of the task state
    /// segment's interrupt stacks, 1 to 7, or 0 for the current stack.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the gate holds the handler's address in three parts"
    )]
    pub const fn interrupt(handler: u64, stack: u8) -> Gate {
        Gate {
            offset_low: handler as u16,
            selector: KERNEL_CODE,
            stack: stack & 7,
            attributes: 0x8e,
            offset_mid: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            reserved: 0,
        }
    }

    /// Where the gate leads.
    #[must_use]
    pub fn handler(&self) -> u64 {
        u64::from(self.offset_high) << 32
            | u64::from(self.offset_mid) << 16
            | u64::from(self.offset_low)
    }

    #[must_use]
    pub fn stack(&self) -> u8 {
        self.stack
    }

    #[must_use]
    pub fn is_present(&self) -> bool {
        self.attributes & 0x80 != 0
    }
}

/// What `lgdt` and `lidt` take, and `sgdt` and `sidt` give back: the last
/// byte's offset and the table's address.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C, packed)]
pub struct Pointer {
    pub limit: u16,
    pub base: u64,
}

impl Pointer {
    /// The pointer to a table of `T`s at `base`.
    #[must_use]
    pub fn to<T>(table: &[T]) -> Pointer {
        Pointer {
            limit: u16::try_from(size_of_val(table) - 1).unwrap_or(u16::MAX),
            base: table.as_ptr() as u64,
        }
    }
}

/// Whether a descriptor is a present 64-bit task state segment, available
/// or busy.
#[must_use]
pub fn is_task_state(descriptor: u64) -> bool {
    descriptor >> 47 & 1 == 1 && matches!(descriptor >> 40 & 0x1f, 0x09 | 0x0b)
}

/// The exceptions that push an error code onto the stack.
#[must_use]
pub fn has_error_code(vector: u8) -> bool {
    matches!(vector, 8 | 10..=14 | 17 | 21 | 29 | 30)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::offset_of;

    #[test]
    fn code_and_data_descriptors() {
        let table = gdt(0);
        assert_eq!(table[0], 0, "the null entry");
        let code = table[usize::from(KERNEL_CODE / 8)];
        assert_eq!(code >> 47 & 1, 1, "present");
        assert_eq!(code >> 45 & 3, 0, "ring 0");
        assert_eq!(code >> 43 & 1, 1, "code");
        assert_eq!(code >> 53 & 1, 1, "64-bit");
        assert_eq!(code >> 54 & 1, 0, "D must be clear in long mode");
        let data = table[usize::from(KERNEL_DATA / 8)];
        assert_eq!(data >> 47 & 1, 1, "present");
        assert_eq!(data >> 43 & 1, 0, "data");
        assert_eq!(data >> 41 & 1, 1, "writable");
        assert_eq!([KERNEL_CODE, KERNEL_DATA, TASK_STATE], [8, 16, 24]);
    }

    /// Base, limit and flags of a code or data descriptor, taken apart.
    fn fields(d: u64) -> (u64, u64, u64) {
        let base = (d >> 16 & 0xff_ffff) | (d >> 56 & 0xff) << 24;
        let limit = (d & 0xffff) | (d >> 48 & 0xf) << 16;
        (base, limit, d >> 52 & 0xf)
    }

    #[test]
    fn code_and_data_span_everything() {
        let table = gdt(0x1234);
        // Base 0 and the largest limit in pages: long mode ignores both
        // for code, but tools that read the table expect them.
        assert_eq!(
            fields(table[1]),
            (0, 0xf_ffff, 0b1010),
            "granular, long mode"
        );
        assert_eq!(
            fields(table[2]),
            (0, 0xf_ffff, 0b1100),
            "granular, 32-bit default"
        );
        assert_eq!(table[1] >> 40 & 1, 0, "not marked accessed yet");
        assert_eq!(table[2] >> 40 & 1, 0);
    }

    #[test]
    fn every_task_state_address_comes_back() {
        for tss in [
            0,
            0x1000,
            0xffff_ffff,
            0x1_0000_0000,
            0xffff_8000_0000_0000,
            0xffff_ffff_8765_4321,
        ] {
            let t = gdt(tss);
            let base = (t[3] >> 16 & 0xff_ffff) | (t[3] >> 56 & 0xff) << 24 | t[4] << 32;
            assert_eq!(base, tss, "{tss:#x}");
            assert_eq!(t[..3], gdt(0)[..3], "only the task state entries change");
        }
    }

    #[test]
    fn the_task_state_descriptor_holds_its_address() {
        let tss = 0xffff_ffff_8012_3456;
        let table = gdt(tss);
        let (low, high) = (table[3], table[4]);
        let base = (low >> 16 & 0xff_ffff) | (low >> 56 & 0xff) << 24 | high << 32;
        assert_eq!(base, tss);
        let limit = (low & 0xffff) | (low >> 48 & 0xf) << 16;
        assert_eq!(limit, 103);
        assert_eq!(
            low >> 40 & 0xff,
            0x89,
            "present, available 64-bit task state"
        );
        assert_eq!(high >> 32, 0, "the upper half's reserved bits");
    }

    #[test]
    fn the_task_state_layout() {
        assert_eq!(size_of::<TaskState>(), usize::from(TASK_STATE_SIZE));
        assert_eq!(offset_of!(TaskState, privilege_stacks), 4);
        assert_eq!(offset_of!(TaskState, interrupt_stacks), 36);
        assert_eq!(offset_of!(TaskState, io_map), 102);
        let t = TaskState::new();
        assert_eq!({ t.io_map }, 104, "no I/O ports for user mode");
        assert_eq!({ t.interrupt_stacks }, [0; 7]);
        assert_eq!(
            TaskState::default().io_map,
            0,
            "only new() closes the ports"
        );
    }

    #[test]
    fn gates_split_and_join_the_handler() {
        assert_eq!(size_of::<Gate>(), 16);
        for handler in [0, 0xffff_ffff_8000_1234, 0x1234_5678_9abc_def0, u64::MAX] {
            let gate = Gate::interrupt(handler, 1);
            assert_eq!(gate.handler(), handler);
            assert!(gate.is_present());
            assert_eq!(gate.stack(), 1);
        }
        let g = Gate::interrupt(0xffff_ffff_8000_1234, 9);
        assert_eq!(g.stack(), 1, "only three bits pick a stack");
        assert_eq!((g.selector, g.attributes), (KERNEL_CODE, 0x8e));
        assert_eq!(
            (g.offset_low, g.offset_mid, g.offset_high),
            (0x1234, 0x8000, 0xffff_ffff)
        );
        for stack in 0..8 {
            assert_eq!(Gate::interrupt(0x1000, stack).stack(), stack);
        }
        assert!(!Gate::MISSING.is_present());
        assert_eq!(Gate::default(), Gate::MISSING);
    }

    #[test]
    fn pointers_say_where_the_last_byte_is() {
        assert_eq!(size_of::<Pointer>(), 10);
        assert_eq!(
            (offset_of!(Pointer, limit), offset_of!(Pointer, base)),
            (0, 2),
            "packed"
        );
        let table = [0u64; 5];
        let p = Pointer::to(&table);
        assert_eq!({ p.limit }, 39);
        assert_eq!({ p.base }, table.as_ptr() as u64);
        let gates = [Gate::MISSING; 256];
        assert_eq!({ Pointer::to(&gates).limit }, 4095);
        for len in 1..=32 {
            let entries = vec![0u64; len];
            assert_eq!(usize::from(Pointer::to(&entries).limit), len * 8 - 1);
        }
    }

    #[test]
    fn task_state_descriptors_are_told_apart() {
        let table = gdt(0xffff_ffff_8000_9000);
        assert!(is_task_state(table[3]), "available");
        assert!(is_task_state(table[3] | 1 << 41), "busy");
        assert!(!is_task_state(table[3] & !(1 << 47)), "not present");
        assert!(!is_task_state(table[0]), "the null entry");
        assert!(
            !is_task_state(table[1]) && !is_task_state(table[2]),
            "code and data"
        );
        assert!(
            !is_task_state(table[3] & !(0x1f << 40) | 0x8c << 40),
            "a call gate"
        );
        assert!(
            !is_task_state(table[3] | 1 << 44),
            "the system bit must be clear"
        );
    }

    #[test]
    fn which_exceptions_push_an_error_code() {
        let with: Vec<u8> = (0..32).filter(|&v| has_error_code(v)).collect();
        assert_eq!(with, [8, 10, 11, 12, 13, 14, 17, 21, 29, 30]);
        assert!(!has_error_code(32) && !has_error_code(255));
    }
}
