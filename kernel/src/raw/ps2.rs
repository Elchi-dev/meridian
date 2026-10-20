//! The keyboard behind the PS/2 controller: set up once, its bytes taken
//! at each of its interrupts and read as keys, which wait until something
//! takes them.

use ps2::{Decoder, Keys, command, config, port, status};

use super::memory::Spin;

static DECODER: Spin<Decoder> = Spin::new(Decoder::new());
static KEYS: Spin<Keys<64>> = Spin::new(Keys::new());

/// How often the status is read while waiting for the controller.
const PATIENCE: u32 = 100_000;

/// The keyboard's command to send keys, and what it answers commands with.
const ENABLE_SCANNING: u8 = 0xf4;
const ACKNOWLEDGE: u8 = 0xfa;

fn status() -> u8 {
    // SAFETY: the status register only reads.
    unsafe { x86::raw::read_port_u8(port::COMMAND) }
}

fn take() -> u8 {
    // SAFETY: the data port hands out the byte that waits, which the
    // caller wanted.
    unsafe { x86::raw::read_port_u8(port::DATA) }
}

/// Writes `byte` to `to` once the controller took the last one.
fn give(to: u16, byte: u8) -> bool {
    let ready = (0..PATIENCE).any(|_| status() & status::INPUT_FULL == 0);
    if ready {
        // SAFETY: the controller's ports, and a command or byte for it or
        // the keyboard the caller means to send.
        unsafe { x86::raw::write_port_u8(to, byte) };
    }
    ready
}

/// The next byte the controller hands out, once one waits.
fn receive() -> Option<u8> {
    (0..PATIENCE)
        .any(|_| status() & status::OUTPUT_FULL != 0)
        .then(take)
}

/// Sets the controller up for the keyboard on its first port, with the
/// keyboard's interrupt on and translation to scan code set 1, and has the
/// keyboard send keys. Says whether it all answered; a machine without
/// PS/2 answers nothing.
pub fn start() -> bool {
    let quiet =
        give(port::COMMAND, command::DISABLE_FIRST) && give(port::COMMAND, command::DISABLE_SECOND);
    if !quiet {
        return false;
    }
    for _ in 0..16 {
        if status() & status::OUTPUT_FULL == 0 {
            break;
        }
        let _ = take();
    }
    let Some(old) = give(port::COMMAND, command::READ_CONFIG)
        .then(receive)
        .flatten()
    else {
        return false;
    };
    let wanted = ps2::keyboard_config(old);
    let tested = give(port::COMMAND, command::WRITE_CONFIG)
        && give(port::DATA, wanted & !config::FIRST_INTERRUPT)
        && give(port::COMMAND, command::SELF_TEST)
        && receive() == Some(ps2::TEST_PASSED);
    // Some controllers forget their configuration in the self test.
    tested
        && give(port::COMMAND, command::WRITE_CONFIG)
        && give(port::DATA, wanted)
        && give(port::COMMAND, command::ENABLE_FIRST)
        && give(port::DATA, ENABLE_SCANNING)
        && receive() == Some(ACKNOWLEDGE)
}

/// Takes the byte the keyboard's interrupt says waits, and keeps the key
/// it ends.
pub fn interrupt() {
    if status() & status::OUTPUT_FULL == 0 {
        return;
    }
    let byte = take();
    if let Some(event) = DECODER.lock().feed(byte) {
        let _ = KEYS.lock().push(event);
    }
}

/// The controller's configuration byte. Only the test image asks.
#[cfg(feature = "qemu-test")]
pub fn configuration() -> Option<u8> {
    give(port::COMMAND, command::READ_CONFIG)
        .then(receive)
        .flatten()
}

/// The oldest key event waiting.
pub fn next() -> Option<ps2::Event> {
    KEYS.lock().pop()
}
