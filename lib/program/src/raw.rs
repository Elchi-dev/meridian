//! What takes unsafe code: the one way into the kernel, `int 0x2e`, and
//! the start of a program, which the linker has to find by its name.

use abi::Call;

/// Makes the system call `call`, and returns what the kernel left in rax
/// and rdx.
pub fn call(call: Call) -> (u64, u64) {
    let (number, [first, second, third, fourth]) = call.encode();
    let (rax, rdx);
    // SAFETY: the kernel keeps every register but rax and rdx, touches
    // only the memory a call names, after checking it is the program's,
    // and runs the call on a stack of its own.
    unsafe {
        core::arch::asm!(
            "int 0x2e",
            inlateout("rax") number => rax,
            in("rdi") first,
            in("rsi") second,
            inlateout("rdx") third => rdx,
            in("r10") fourth,
            options(nostack),
        );
    }
    (rax, rdx)
}

/// Makes `main`, a `fn(Handle) -> u64`, the program's main function: the
/// kernel starts the program with a handle, and it ends with the code main
/// returns. Also gives the program its panic handler.
#[macro_export]
macro_rules! main {
    ($main:path) => {
        #[cfg(not(test))]
        extern "C" fn __program_main(argument: u64) -> ! {
            $crate::exit($main($crate::Handle(argument)))
        }

        // The kernel enters with the stack pointer at the top of the
        // stack; the call leaves it as every function expects to find it.
        #[cfg(not(test))]
        #[allow(unsafe_code)]
        #[unsafe(no_mangle)]
        #[unsafe(naked)]
        extern "C" fn _start() -> ! {
            core::arch::naked_asm!(
                "and rsp, -16",
                "call {main}",
                "ud2",
                main = sym __program_main,
            )
        }

        #[cfg(not(test))]
        #[panic_handler]
        fn panic(info: &core::panic::PanicInfo) -> ! {
            $crate::panicked(info)
        }
    };
}
