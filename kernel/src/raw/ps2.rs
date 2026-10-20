//! The keyboard behind the PS/2 controller: set up once, its bytes taken
//! at each of its interrupts and read as keys, which wait until something
//! takes them.

use ps2::{Decoder, Keys, Step, port, status};

use super::memory::Spin;

static DECODER: Spin<Decoder> = Spin::new(Decoder::new());
static KEYS: Spin<Keys<64>> = Spin::new(Keys::new());

/// How often the status is read while waiting for the controller.
const PATIENCE: u32 = 100_000;

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

/// Sets the controller up for the keyboard on its first port, step by
/// step as [`ps2::SETUP`] lists them, and says whether every step went
/// through; a machine without PS/2 answers nothing.
pub fn start() -> bool {
    let mut old = None;
    for step in ps2::SETUP {
        let done = match step {
            Step::Command(c) => give(port::COMMAND, c),
            Step::Data(d) => give(port::DATA, d),
            Step::Flush => {
                for _ in 0..16 {
                    if status() & status::OUTPUT_FULL != 0 {
                        let _ = take();
                    }
                }
                true
            }
            Step::ReadConfig => {
                old = receive();
                old.is_some()
            }
            Step::WriteConfig { quiet } => {
                old.is_some_and(|o| give(port::DATA, ps2::config_to_write(o, quiet)))
            }
            Step::Expect(byte) => receive() == Some(byte),
        };
        if !done {
            return false;
        }
    }
    true
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
    give(port::COMMAND, ps2::command::READ_CONFIG)
        .then(receive)
        .flatten()
}

/// The oldest key event waiting.
pub fn next() -> Option<ps2::Event> {
    KEYS.lock().pop()
}
