//! What programs and the kernel agree on. System calls: user code puts the
//! call's number in rax and its arguments in rdi, rsi, rdx and r10, runs
//! `int 0x2e`, and finds the answer in rax. Answers from the top of the
//! range are errors.

#![cfg_attr(not(test), no_std)]

/// Ends the calling thread; rdi is its exit code.
pub const EXIT: u64 = 0;
/// Writes a line to the kernel's log: rdi bytes at rsi are its text, and
/// any byte but printable ASCII shows as a question mark.
pub const LOG: u64 = 1;
/// Lets other threads of the same or a higher priority run first.
pub const YIELD: u64 = 2;
/// Sleeps for rdi timer ticks.
pub const SLEEP: u64 = 3;
/// Answers how many times the timer has ticked.
pub const TICKS: u64 = 4;
/// Makes a channel: the handle of one end comes back in rax, the other's
/// in rdx.
pub const CHANNEL: u64 = 5;
/// Sends rdx bytes at rsi through the channel end of handle rdi, handing
/// over handle r10 with them unless r10 is 0.
pub const SEND: u64 = 6;
/// Takes the next message for the channel end of handle rdi into the rdx
/// bytes at rsi, and answers its length; a handle it brought comes back in
/// rdx, or 0.
pub const RECEIVE: u64 = 7;
/// Closes handle rdi.
pub const CLOSE: u64 = 8;
/// Gives the program rdi fresh pages of memory, zeroed, writable and not
/// executable, and answers where they start.
pub const MAP: u64 = 9;
/// Gives back the rsi pages at rdi that [`MAP`] gave.
pub const UNMAP: u64 = 10;
/// Starts the program whose file is named by the rdi bytes at rsi, handing
/// it handle rdx unless rdx is 0, and answers a handle to its process. The
/// name is a program's the loader read, or a path from the root, at most
/// [`PATH_BYTES`] long, to a file the caller's token may run. It runs with
/// the caller's security token, or with less when r10 is not 0 but a
/// restriction as `lib/security` packs one into a word.
pub const SPAWN: u64 = 11;
/// Waits until the process of handle rdi ended, and answers its exit code,
/// with rdx 0, or the vector of the exception that ended it, with rdx 1.
pub const WAIT: u64 = 12;
/// Writes the caller's security token, in the form `lib/security` reads,
/// into the rdi bytes at rsi, and answers how many bytes it took.
pub const TOKEN: u64 = 13;
/// Opens the file at the path of the rdi bytes at rsi for the rights rdx,
/// those of [`file`], and answers a handle to it. r10 says what to do by
/// whether it is there, one of [`file::CREATE_NEW`] and the four after
/// it; 0 opens it only when it is there, as [`file::OPEN_EXISTING`] does.
pub const OPEN: u64 = 14;
/// Reads at most rdx bytes of the file of handle rdi, from its byte r10
/// on, into the room at rsi, and answers how many it read: 0 at its end.
pub const READ: u64 = 15;
/// Answers the size of the file of handle rdi, with rdx 1 when it is a
/// directory and 0 when not.
pub const INFO: u64 = 16;
/// Writes the name of entry r10 of the directory of handle rdi, counted
/// from 0, into the rdx bytes at rsi in UTF-8, and answers its length,
/// with rdx 1 when the entry is a directory and 0 when not.
pub const LIST: u64 = 17;
/// Writes the rdi bytes at rsi, which have to be UTF-8, to the console,
/// at most [`WRITE_BYTES`] of them.
pub const WRITE: u64 = 18;
/// Answers the next key the keyboard sent: its USB HID usage in the low
/// 16 bits, and 1 << 16 when it went down. Empty when none waits.
pub const KEY: u64 = 19;
/// Ends the process of handle rdi, which has to allow writing, with the
/// exit code rsi, at its next way back to ring 3: once a call it is in is
/// over, or at the next interrupt, or while it waits for another process.
/// A process that ended already stays as it ended.
pub const TERMINATE: u64 = 20;
/// Starts another thread of the caller's program, in ring 3 at rdi with
/// its stack pointer at rsi and rdx in rdi, and answers 0. It ends with
/// [`END_THREAD`], and the program once its last thread ended.
pub const THREAD: u64 = 21;
/// Ends the calling thread. Should it be its program's last, the program
/// exits with the code rdi; [`EXIT`] ends every thread of it at once.
pub const END_THREAD: u64 = 22;
/// Writes the rdx bytes at rsi, at most [`WRITE_BYTES`], into the file of
/// handle rdi from its byte r10 on, which may lie past its end, and
/// answers how many it wrote: all of them. The file grows to hold them.
pub const WRITE_FILE: u64 = 23;
/// Removes the file or the empty directory at the path of the rdi bytes
/// at rsi.
pub const REMOVE: u64 = 24;
/// Makes a directory at the path of the rdi bytes at rsi.
pub const MAKE_DIRECTORY: u64 = 25;
/// Answers a second handle to what the handle in rdi stands for, with
/// the same rights (decision record 0019).
pub const DUPLICATE: u64 = 26;
/// Moves the file or directory at the path of the rdi bytes at rsi to the
/// path of the rdx bytes at r10, on the same volume.
pub const RENAME: u64 = 27;

/// The most bytes one WRITE takes.
pub const WRITE_BYTES: usize = 4096;

/// The rights to a file, as Windows numbers them.
pub mod file {
    pub const READ: u32 = 1;
    pub const WRITE: u32 = 1 << 1;
    pub const APPEND: u32 = 1 << 2;
    pub const RUN: u32 = 1 << 5;

    /// What OPEN does by whether the file is there, as Windows numbers it:
    /// make it, failing when it is there; make it, emptying one that is;
    /// open it only when it is there; open it, making it when it is not;
    /// and empty it, only when it is there.
    pub const CREATE_NEW: u64 = 1;
    pub const CREATE_ALWAYS: u64 = 2;
    pub const OPEN_EXISTING: u64 = 3;
    pub const OPEN_ALWAYS: u64 = 4;
    pub const TRUNCATE_EXISTING: u64 = 5;
}

/// Why a call failed. Each error is an answer counting down from the top
/// of the range, so no address, length or handle is mistaken for one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The call does not exist.
    NoSuchCall,
    /// An address the caller gave is not its own to give.
    BadAddress,
    /// A line is longer than [`LOG_MAX`], or a message longer than a
    /// channel takes.
    TooLong,
    /// No handle of the caller's has this number.
    BadHandle,
    /// The handle does not allow what was asked, or stands for something
    /// else.
    Denied,
    /// The channel holds as many messages as it takes.
    Full,
    /// No message waits.
    Empty,
    /// The other end of the channel is closed.
    Closed,
    /// The message waiting is longer than the room given for it; it stays.
    TooSmall,
    /// No room is left for another channel or handle, or no memory for
    /// more pages.
    NoRoom,
    /// The count of pages is 0, or more than one call takes.
    BadSize,
    /// No program file has this name.
    NoSuchProgram,
    /// The file is no `.mexec` file this kernel runs.
    NotAProgram,
    /// A value the call does not take, such as a restriction that is none.
    BadArgument,
    /// No file has this path.
    NotFound,
    /// The handle stands for a directory, which has no bytes to read.
    NotAFile,
    /// The disk a file is on could not be read.
    Unreadable,
    /// The handle stands for a file, which has no entries to list.
    NotADirectory,
    /// A file or directory has the path already.
    Exists,
    /// The volume the path is on is mounted read only.
    ReadOnly,
    /// No room is left on the volume.
    DiskFull,
    /// The directory holds entries, so it stays.
    NotEmpty,
    /// The disk a file is on could not be written.
    Unwritable,
    /// The file is open, so it stays.
    InUse,
    /// A directory would move into itself, or into a directory inside it.
    Inside,
    /// The two paths of a move are on different volumes.
    NotSameVolume,
}

const ERRORS: [Error; 26] = [
    Error::NoSuchCall,
    Error::BadAddress,
    Error::TooLong,
    Error::BadHandle,
    Error::Denied,
    Error::Full,
    Error::Empty,
    Error::Closed,
    Error::TooSmall,
    Error::NoRoom,
    Error::BadSize,
    Error::NoSuchProgram,
    Error::NotAProgram,
    Error::BadArgument,
    Error::NotFound,
    Error::NotAFile,
    Error::Unreadable,
    Error::NotADirectory,
    Error::Exists,
    Error::ReadOnly,
    Error::DiskFull,
    Error::NotEmpty,
    Error::Unwritable,
    Error::InUse,
    Error::Inside,
    Error::NotSameVolume,
];

impl Error {
    /// The answer in rax that stands for this error.
    #[must_use]
    pub const fn code(self) -> u64 {
        u64::MAX - self as u64
    }
}

/// Reads the answer a call left in rax.
///
/// # Errors
///
/// The error the answer stands for, when it is one of [`Error`]'s codes.
pub fn answer(rax: u64) -> Result<u64, Error> {
    match ERRORS.into_iter().find(|e| e.code() == rax) {
        Some(error) => Err(error),
        None => Ok(rax),
    }
}

pub const NO_SUCH_CALL: u64 = Error::NoSuchCall.code();
pub const BAD_ADDRESS: u64 = Error::BadAddress.code();
pub const TOO_LONG: u64 = Error::TooLong.code();
pub const BAD_HANDLE: u64 = Error::BadHandle.code();
pub const DENIED: u64 = Error::Denied.code();
pub const FULL: u64 = Error::Full.code();
pub const EMPTY: u64 = Error::Empty.code();
pub const CLOSED: u64 = Error::Closed.code();
pub const TOO_SMALL: u64 = Error::TooSmall.code();
pub const NO_ROOM: u64 = Error::NoRoom.code();
pub const BAD_SIZE: u64 = Error::BadSize.code();
pub const NO_SUCH_PROGRAM: u64 = Error::NoSuchProgram.code();
pub const NOT_A_PROGRAM: u64 = Error::NotAProgram.code();
pub const BAD_ARGUMENT: u64 = Error::BadArgument.code();
pub const NOT_FOUND: u64 = Error::NotFound.code();
pub const NOT_A_FILE: u64 = Error::NotAFile.code();
pub const UNREADABLE: u64 = Error::Unreadable.code();
pub const NOT_A_DIRECTORY: u64 = Error::NotADirectory.code();
pub const EXISTS: u64 = Error::Exists.code();
pub const READ_ONLY: u64 = Error::ReadOnly.code();
pub const DISK_FULL: u64 = Error::DiskFull.code();
pub const NOT_EMPTY: u64 = Error::NotEmpty.code();
pub const UNWRITABLE: u64 = Error::Unwritable.code();
pub const IN_USE: u64 = Error::InUse.code();
pub const INSIDE: u64 = Error::Inside.code();
pub const NOT_SAME_VOLUME: u64 = Error::NotSameVolume.code();

/// What makes an ELF file a Meridian program, a `.mexec` file: a note
/// whose owner is [`NOTE_OWNER`], of the kind [`NOTE_VERSION`], whose four
/// bytes hold [`MEXEC_VERSION`]. A note of the kind [`NOTE_NAME`] holds the
/// program's name, in UTF-8.
pub const NOTE_OWNER: &[u8] = b"Meridian";
pub const NOTE_VERSION: u32 = 1;
pub const NOTE_NAME: u32 = 2;
pub const MEXEC_VERSION: u32 = 1;

/// The longest message a channel takes, in bytes.
pub const MESSAGE_BYTES: usize = 128;

/// The longest path a call takes, in bytes.
pub const PATH_BYTES: usize = 512;

/// The longest line the log takes, in bytes.
pub const LOG_BYTES: usize = 256;
pub const LOG_MAX: u64 = LOG_BYTES as u64;

/// Where the lower half, the one user code has, ends.
pub const USER_END: u64 = 0x0000_8000_0000_0000;

/// A system call, read from the registers it came in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    Exit(u64),
    Log {
        len: u64,
        at: u64,
    },
    Yield,
    Sleep(u64),
    Ticks,
    Channel,
    Send {
        handle: u64,
        at: u64,
        len: u64,
        carry: u64,
    },
    Receive {
        handle: u64,
        at: u64,
        room: u64,
    },
    Close(u64),
    Map(u64),
    Unmap {
        at: u64,
        pages: u64,
    },
    Spawn {
        len: u64,
        at: u64,
        carry: u64,
        restriction: u64,
    },
    Wait(u64),
    Token {
        room: u64,
        at: u64,
    },
    Open {
        len: u64,
        at: u64,
        wanted: u64,
        how: u64,
    },
    Read {
        handle: u64,
        at: u64,
        room: u64,
        offset: u64,
    },
    Info(u64),
    List {
        handle: u64,
        at: u64,
        room: u64,
        index: u64,
    },
    Write {
        len: u64,
        at: u64,
    },
    Key,
    Terminate {
        handle: u64,
        code: u64,
    },
    Thread {
        entry: u64,
        stack: u64,
        argument: u64,
    },
    EndThread(u64),
    WriteFile {
        handle: u64,
        at: u64,
        len: u64,
        offset: u64,
    },
    Remove {
        len: u64,
        at: u64,
    },
    MakeDirectory {
        len: u64,
        at: u64,
    },
    Duplicate(u64),
    Rename {
        from_len: u64,
        from_at: u64,
        to_len: u64,
        to_at: u64,
    },
    Unknown(u64),
}

impl Call {
    /// The number and the registers rdi, rsi, rdx and r10 that ask for
    /// this call, the way [`decode`] reads them. Registers the call does
    /// not use are 0, so a later call can give them a meaning.
    #[must_use]
    pub fn encode(self) -> (u64, [u64; 4]) {
        match self {
            Call::Exit(code) => (EXIT, [code, 0, 0, 0]),
            Call::Log { len, at } => (LOG, [len, at, 0, 0]),
            Call::Yield => (YIELD, [0; 4]),
            Call::Sleep(ticks) => (SLEEP, [ticks, 0, 0, 0]),
            Call::Ticks => (TICKS, [0; 4]),
            Call::Channel => (CHANNEL, [0; 4]),
            Call::Send {
                handle,
                at,
                len,
                carry,
            } => (SEND, [handle, at, len, carry]),
            Call::Receive { handle, at, room } => (RECEIVE, [handle, at, room, 0]),
            Call::Close(handle) => (CLOSE, [handle, 0, 0, 0]),
            Call::Map(pages) => (MAP, [pages, 0, 0, 0]),
            Call::Unmap { at, pages } => (UNMAP, [at, pages, 0, 0]),
            Call::Spawn {
                len,
                at,
                carry,
                restriction,
            } => (SPAWN, [len, at, carry, restriction]),
            Call::Wait(handle) => (WAIT, [handle, 0, 0, 0]),
            Call::Token { room, at } => (TOKEN, [room, at, 0, 0]),
            Call::Open {
                len,
                at,
                wanted,
                how,
            } => (OPEN, [len, at, wanted, how]),
            Call::Read {
                handle,
                at,
                room,
                offset,
            } => (READ, [handle, at, room, offset]),
            Call::Info(handle) => (INFO, [handle, 0, 0, 0]),
            Call::List {
                handle,
                at,
                room,
                index,
            } => (LIST, [handle, at, room, index]),
            Call::Write { len, at } => (WRITE, [len, at, 0, 0]),
            Call::Key => (KEY, [0; 4]),
            Call::Terminate { handle, code } => (TERMINATE, [handle, code, 0, 0]),
            Call::Thread {
                entry,
                stack,
                argument,
            } => (THREAD, [entry, stack, argument, 0]),
            Call::EndThread(code) => (END_THREAD, [code, 0, 0, 0]),
            Call::WriteFile {
                handle,
                at,
                len,
                offset,
            } => (WRITE_FILE, [handle, at, len, offset]),
            Call::Remove { len, at } => (REMOVE, [len, at, 0, 0]),
            Call::MakeDirectory { len, at } => (MAKE_DIRECTORY, [len, at, 0, 0]),
            Call::Duplicate(handle) => (DUPLICATE, [handle, 0, 0, 0]),
            Call::Rename {
                from_len,
                from_at,
                to_len,
                to_at,
            } => (RENAME, [from_len, from_at, to_len, to_at]),
            Call::Unknown(number) => (number, [0; 4]),
        }
    }
}

/// The call `number` asks for, with `args` as rdi, rsi, rdx and r10.
#[must_use]
pub fn decode(number: u64, args: [u64; 4]) -> Call {
    let [first, second, third, fourth] = args;
    match number {
        EXIT => Call::Exit(first),
        LOG => Call::Log {
            len: first,
            at: second,
        },
        YIELD => Call::Yield,
        SLEEP => Call::Sleep(first),
        TICKS => Call::Ticks,
        CHANNEL => Call::Channel,
        SEND => Call::Send {
            handle: first,
            at: second,
            len: third,
            carry: fourth,
        },
        RECEIVE => Call::Receive {
            handle: first,
            at: second,
            room: third,
        },
        CLOSE => Call::Close(first),
        MAP => Call::Map(first),
        UNMAP => Call::Unmap {
            at: first,
            pages: second,
        },
        SPAWN => Call::Spawn {
            len: first,
            at: second,
            carry: third,
            restriction: fourth,
        },
        WAIT => Call::Wait(first),
        TOKEN => Call::Token {
            room: first,
            at: second,
        },
        OPEN => Call::Open {
            len: first,
            at: second,
            wanted: third,
            how: fourth,
        },
        READ => Call::Read {
            handle: first,
            at: second,
            room: third,
            offset: fourth,
        },
        INFO => Call::Info(first),
        LIST => Call::List {
            handle: first,
            at: second,
            room: third,
            index: fourth,
        },
        WRITE => Call::Write {
            len: first,
            at: second,
        },
        KEY => Call::Key,
        TERMINATE => Call::Terminate {
            handle: first,
            code: second,
        },
        THREAD => Call::Thread {
            entry: first,
            stack: second,
            argument: third,
        },
        END_THREAD => Call::EndThread(first),
        WRITE_FILE => Call::WriteFile {
            handle: first,
            at: second,
            len: third,
            offset: fourth,
        },
        REMOVE => Call::Remove {
            len: first,
            at: second,
        },
        MAKE_DIRECTORY => Call::MakeDirectory {
            len: first,
            at: second,
        },
        DUPLICATE => Call::Duplicate(first),
        RENAME => Call::Rename {
            from_len: first,
            from_at: second,
            to_len: third,
            to_at: fourth,
        },
        other => Call::Unknown(other),
    }
}

/// Whether the `len` bytes from `at` lie in the lower half, the user's,
/// without wrapping around.
#[must_use]
pub fn in_user_half(at: u64, len: u64) -> bool {
    at.checked_add(len).is_some_and(|end| end <= USER_END)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_come_from_their_registers() {
        assert_eq!(decode(EXIT, [3, 9, 8, 0]), Call::Exit(3));
        assert_eq!(
            decode(LOG, [12, 0x40_0100, 8, 0]),
            Call::Log {
                len: 12,
                at: 0x40_0100
            }
        );
        assert_eq!(decode(YIELD, [1, 2, 3, 0]), Call::Yield);
        assert_eq!(decode(SLEEP, [25, 2, 3, 0]), Call::Sleep(25));
        assert_eq!(decode(TICKS, [1, 2, 3, 0]), Call::Ticks);
        assert_eq!(decode(CHANNEL, [1, 2, 3, 0]), Call::Channel);
        assert_eq!(
            decode(SEND, [0x1_0001, 0x7f_f000, 4, 0x2_0002]),
            Call::Send {
                handle: 0x1_0001,
                at: 0x7f_f000,
                len: 4,
                carry: 0x2_0002
            }
        );
        assert_eq!(
            decode(RECEIVE, [2, 0x7f_f100, 128, 0]),
            Call::Receive {
                handle: 2,
                at: 0x7f_f100,
                room: 128
            }
        );
        assert_eq!(decode(CLOSE, [7, 2, 3, 0]), Call::Close(7));
        assert_eq!(decode(DUPLICATE, [9, 2, 3, 4]), Call::Duplicate(9));
        assert_eq!(
            decode(RENAME, [5, 0x40_1000, 6, 0x40_2000]),
            Call::Rename {
                from_len: 5,
                from_at: 0x40_1000,
                to_len: 6,
                to_at: 0x40_2000
            }
        );
        assert_eq!(decode(MAP, [16, 2, 3, 0]), Call::Map(16));
        assert_eq!(
            decode(UNMAP, [0x10_0000_0000, 4, 3, 0]),
            Call::Unmap {
                at: 0x10_0000_0000,
                pages: 4
            }
        );
        assert_eq!(
            decode(SPAWN, [11, 0x40_2000, 0x1_0003, 0x8000_0001_0000_0002]),
            Call::Spawn {
                len: 11,
                at: 0x40_2000,
                carry: 0x1_0003,
                restriction: 0x8000_0001_0000_0002
            }
        );
        assert_eq!(decode(WAIT, [0x2_0001, 2, 3, 0]), Call::Wait(0x2_0001));
        assert_eq!(
            decode(TOKEN, [1200, 0x7f_e000, 3, 4]),
            Call::Token {
                room: 1200,
                at: 0x7f_e000
            }
        );
        assert_eq!(decode(28, [1, 2, 3, 0]), Call::Unknown(28));
        assert_eq!(decode(u64::MAX, [1, 2, 3, 0]), Call::Unknown(u64::MAX));
        assert_eq!(
            decode(THREAD, [0x40_1000, 0x10_0000_4000, 7, 4]),
            Call::Thread {
                entry: 0x40_1000,
                stack: 0x10_0000_4000,
                argument: 7
            }
        );
        assert_eq!(decode(END_THREAD, [3, 2, 1, 0]), Call::EndThread(3));
        assert_eq!(
            decode(TERMINATE, [0x5_0001, 77, 3, 4]),
            Call::Terminate {
                handle: 0x5_0001,
                code: 77
            }
        );
    }

    #[test]
    fn each_call_has_its_number() {
        assert_eq!(
            [
                EXIT,
                LOG,
                YIELD,
                SLEEP,
                TICKS,
                CHANNEL,
                SEND,
                RECEIVE,
                CLOSE,
                MAP,
                UNMAP,
                SPAWN,
                WAIT,
                TOKEN,
                OPEN,
                READ,
                INFO,
                LIST,
                WRITE,
                KEY,
                TERMINATE,
                THREAD,
                END_THREAD,
                WRITE_FILE,
                REMOVE,
                MAKE_DIRECTORY,
                DUPLICATE,
                RENAME
            ],
            [
                0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22,
                23, 24, 25, 26, 27
            ]
        );
    }

    #[test]
    fn calls_for_files_and_the_console_come_from_their_registers() {
        assert_eq!(
            decode(OPEN, [30, 0x7f_e000, 1, 4]),
            Call::Open {
                len: 30,
                at: 0x7f_e000,
                wanted: 1,
                how: 4
            }
        );
        assert_eq!(
            decode(WRITE_FILE, [0x3_0001, 0x7f_d000, 4096, 1 << 33]),
            Call::WriteFile {
                handle: 0x3_0001,
                at: 0x7f_d000,
                len: 4096,
                offset: 1 << 33
            }
        );
        assert_eq!(
            decode(REMOVE, [9, 0x7f_c000, 3, 4]),
            Call::Remove {
                len: 9,
                at: 0x7f_c000
            }
        );
        assert_eq!(
            decode(MAKE_DIRECTORY, [10, 0x7f_b000, 3, 4]),
            Call::MakeDirectory {
                len: 10,
                at: 0x7f_b000
            }
        );
        assert_eq!(
            decode(READ, [0x3_0001, 0x7f_d000, 4096, 512]),
            Call::Read {
                handle: 0x3_0001,
                at: 0x7f_d000,
                room: 4096,
                offset: 512
            }
        );
        assert_eq!(decode(INFO, [0x3_0001, 2, 3, 4]), Call::Info(0x3_0001));
        assert_eq!(
            decode(LIST, [0x3_0002, 0x7f_d000, 1024, 7]),
            Call::List {
                handle: 0x3_0002,
                at: 0x7f_d000,
                room: 1024,
                index: 7
            }
        );
        assert_eq!(
            decode(WRITE, [5, 0x7f_a000, 3, 4]),
            Call::Write {
                len: 5,
                at: 0x7f_a000
            }
        );
        assert_eq!(decode(KEY, [1, 2, 3, 4]), Call::Key);
    }

    #[test]
    fn spawn_hands_its_restriction_over_bit_for_bit() {
        for word in [0, 1, 1 << 63, 0x8000_0004_ffff_ffff, u64::MAX] {
            let Call::Spawn { restriction, .. } = decode(SPAWN, [1, 2, 3, word]) else {
                panic!("SPAWN should read as a spawn");
            };
            assert_eq!(restriction, word);
        }
    }

    #[test]
    fn read_takes_its_offset_whole_from_r10() {
        for offset in [0, 1, 4096, 1 << 32, u64::MAX] {
            let Call::Read { offset: read, .. } = decode(READ, [1, 2, 3, offset]) else {
                panic!("READ should read as a read");
            };
            assert_eq!(read, offset);
        }
    }

    #[test]
    fn list_takes_its_registers_as_read_does() {
        for last in [0, 1, 255, 1 << 40, u64::MAX] {
            let args = [0x3_0002, 0x7f_d000, 64, last];
            let Call::Read {
                handle,
                at,
                room,
                offset,
            } = decode(READ, args)
            else {
                panic!("READ should read as a read");
            };
            let index = offset;
            assert_eq!(
                decode(LIST, args),
                Call::List {
                    handle,
                    at,
                    room,
                    index
                },
                "{last}"
            );
        }
    }

    #[test]
    fn token_takes_its_room_first_as_log_takes_its_length() {
        let Call::Log { len, at } = decode(LOG, [64, 0x7f_e000, 0, 0]) else {
            panic!("LOG should read as a log line");
        };
        assert_eq!(
            decode(TOKEN, [64, 0x7f_e000, 0, 0]),
            Call::Token { room: len, at }
        );
    }

    #[test]
    fn the_newest_error_comes_last_so_the_others_keep_their_codes() {
        assert_eq!(NOT_A_PROGRAM, u64::MAX - 12);
        assert_eq!(BAD_ARGUMENT, u64::MAX - 13);
        assert_eq!(NOT_A_FILE, u64::MAX - 15);
        assert_eq!(UNREADABLE, u64::MAX - 16);
        assert_eq!(NOT_A_DIRECTORY, u64::MAX - 17);
        assert_eq!(EXISTS, u64::MAX - 18);
        assert_eq!(UNWRITABLE, u64::MAX - 22);
        assert_eq!(IN_USE, u64::MAX - 23);
        assert_eq!(INSIDE, u64::MAX - 24);
        assert_eq!(NOT_SAME_VOLUME, u64::MAX - 25);
        assert_eq!(ERRORS.last(), Some(&Error::NotSameVolume));
    }

    #[test]
    fn file_rights_are_windows_own() {
        assert_eq!(
            [file::READ, file::WRITE, file::APPEND, file::RUN],
            [1, 2, 4, 0x20]
        );
        assert_eq!(
            [
                file::CREATE_NEW,
                file::CREATE_ALWAYS,
                file::OPEN_EXISTING,
                file::OPEN_ALWAYS,
                file::TRUNCATE_EXISTING
            ],
            [1, 2, 3, 4, 5]
        );
    }

    #[test]
    fn errors_sit_at_the_top_where_no_answer_reaches() {
        let errors = [
            NO_SUCH_CALL,
            BAD_ADDRESS,
            TOO_LONG,
            BAD_HANDLE,
            DENIED,
            FULL,
            EMPTY,
            CLOSED,
            TOO_SMALL,
            NO_ROOM,
            BAD_SIZE,
            NO_SUCH_PROGRAM,
            NOT_A_PROGRAM,
            BAD_ARGUMENT,
            NOT_FOUND,
            NOT_A_FILE,
            UNREADABLE,
            NOT_A_DIRECTORY,
            EXISTS,
            READ_ONLY,
            DISK_FULL,
            NOT_EMPTY,
            UNWRITABLE,
            IN_USE,
            INSIDE,
            NOT_SAME_VOLUME,
        ];
        assert_eq!(errors.len(), ERRORS.len());
        for (i, &e) in errors.iter().enumerate() {
            assert_eq!(
                e,
                u64::MAX - i as u64,
                "each its own, counting down from the top"
            );
            assert!(
                e > USER_END,
                "no address, length or handle is mistaken for an error"
            );
        }
    }

    /// Says each of `calls` reads back from its registers as it was.
    fn reads_back(calls: &[Call]) {
        for &call in calls {
            let (number, args) = call.encode();
            assert_eq!(
                decode(number, args),
                call,
                "{call:?} as {number}, {args:x?}"
            );
        }
    }

    #[test]
    fn every_call_reads_back_as_it_was_written() {
        let calls = [
            Call::Exit(3),
            Call::Log {
                len: 12,
                at: 0x40_0100,
            },
            Call::Yield,
            Call::Sleep(25),
            Call::Ticks,
            Call::Channel,
            Call::Send {
                handle: 0x1_0001,
                at: 0x7f_f000,
                len: 4,
                carry: 0x2_0002,
            },
            Call::Receive {
                handle: 2,
                at: 0x7f_f100,
                room: 128,
            },
            Call::Close(7),
            Call::Map(32),
            Call::Unmap {
                at: 0x10_0000_2000,
                pages: 3,
            },
            Call::Spawn {
                len: 5,
                at: 0x40_3000,
                carry: 0x3_0002,
                restriction: 0x8000_0000_0000_0000,
            },
            Call::Wait(0x4_0001),
            Call::Token {
                room: 64,
                at: 0x7f_e000,
            },
        ];
        reads_back(&calls);
    }

    #[test]
    fn every_call_for_files_reads_back_as_it_was_written() {
        let calls = [
            Call::Open {
                len: 9,
                at: 0x40_4000,
                wanted: 0x21,
                how: file::OPEN_ALWAYS,
            },
            Call::Read {
                handle: 0x3_0004,
                at: 0x7f_c000,
                room: 512,
                offset: 1 << 40,
            },
            Call::Info(0x3_0004),
            Call::List {
                handle: 0x3_0005,
                at: 0x7f_b000,
                room: 64,
                index: 300,
            },
            Call::Write {
                len: 12,
                at: 0x40_5000,
            },
            Call::Key,
            Call::Terminate {
                handle: 0x4_0002,
                code: u64::MAX,
            },
            Call::Thread {
                entry: 0x40_6000,
                stack: 0x10_0000_8000,
                argument: u64::MAX,
            },
            Call::EndThread(1 << 63),
            Call::WriteFile {
                handle: 0x3_0006,
                at: 0x7f_a000,
                len: 100,
                offset: u64::MAX,
            },
            Call::Remove {
                len: 7,
                at: 0x40_7000,
            },
            Call::MakeDirectory {
                len: 8,
                at: 0x40_8000,
            },
            Call::Duplicate(0x1_0003),
            Call::Rename {
                from_len: 9,
                from_at: 0x40_9000,
                to_len: 10,
                to_at: 0x40_a000,
            },
            Call::Unknown(28),
        ];

        reads_back(&calls);
    }

    #[test]
    fn each_call_reads_exactly_the_registers_it_names() {
        let used = [
            (EXIT, 1),
            (LOG, 2),
            (YIELD, 0),
            (SLEEP, 1),
            (TICKS, 0),
            (CHANNEL, 0),
            (SEND, 4),
            (RECEIVE, 3),
            (CLOSE, 1),
            (MAP, 1),
            (UNMAP, 2),
            (SPAWN, 4),
            (WAIT, 1),
            (TOKEN, 2),
            (OPEN, 4),
            (READ, 4),
            (INFO, 1),
            (LIST, 4),
            (WRITE, 2),
            (KEY, 0),
            (TERMINATE, 2),
            (THREAD, 3),
            (END_THREAD, 1),
            (WRITE_FILE, 4),
            (REMOVE, 2),
            (MAKE_DIRECTORY, 2),
            (DUPLICATE, 1),
            (RENAME, 4),
        ];
        let args = [11, 22, 33, 44];
        for (number, count) in used {
            let call = decode(number, args);
            for register in 0..4 {
                let mut changed = args;
                changed[register] += 100;
                assert_eq!(
                    decode(number, changed) != call,
                    register < count,
                    "call {number}, register {register}"
                );
            }
        }
    }

    #[test]
    fn the_calls_are_numbered_without_gaps() {
        for number in 0..64 {
            let call = decode(number, [0; 4]);
            assert_eq!(
                call == Call::Unknown(number),
                number > RENAME,
                "call {number}"
            );
            assert_eq!(call.encode(), (number, [0; 4]), "call {number}");
        }
        for error in ERRORS {
            assert_eq!(decode(error.code(), [0; 4]), Call::Unknown(error.code()));
        }
    }

    #[test]
    fn calls_leave_the_registers_they_do_not_use_at_zero() {
        assert_eq!(Call::Exit(3).encode(), (EXIT, [3, 0, 0, 0]));
        assert_eq!(Call::Log { len: 5, at: 6 }.encode(), (LOG, [5, 6, 0, 0]));
        assert_eq!(Call::Yield.encode(), (YIELD, [0; 4]));
        assert_eq!(Call::Sleep(1).encode(), (SLEEP, [1, 0, 0, 0]));
        assert_eq!(Call::Ticks.encode(), (TICKS, [0; 4]));
        assert_eq!(Call::Channel.encode(), (CHANNEL, [0; 4]));
        let receive = Call::Receive {
            handle: 1,
            at: 2,
            room: 3,
        };
        assert_eq!(receive.encode(), (RECEIVE, [1, 2, 3, 0]));
        assert_eq!(Call::Close(4).encode(), (CLOSE, [4, 0, 0, 0]));
        assert_eq!(Call::Duplicate(6).encode(), (DUPLICATE, [6, 0, 0, 0]));
        assert_eq!(Call::Map(5).encode(), (MAP, [5, 0, 0, 0]));
        assert_eq!(
            Call::Unmap { at: 6, pages: 7 }.encode(),
            (UNMAP, [6, 7, 0, 0])
        );
        assert_eq!(Call::Wait(8).encode(), (WAIT, [8, 0, 0, 0]));
        let spawn = Call::Spawn {
            len: 9,
            at: 10,
            carry: 11,
            restriction: 0,
        };
        assert_eq!(spawn.encode(), (SPAWN, [9, 10, 11, 0]));
        let token = Call::Token { room: 12, at: 13 };
        assert_eq!(token.encode(), (TOKEN, [12, 13, 0, 0]));
        let open = Call::Open {
            len: 14,
            at: 15,
            wanted: 16,
            how: 0,
        };
        assert_eq!(open.encode(), (OPEN, [14, 15, 16, 0]));
        assert_eq!(Call::Info(17).encode(), (INFO, [17, 0, 0, 0]));
        let write = Call::Write { len: 18, at: 19 };
        assert_eq!(write.encode(), (WRITE, [18, 19, 0, 0]));
        assert_eq!(Call::Key.encode(), (KEY, [0; 4]));
        let terminate = Call::Terminate {
            handle: 20,
            code: 21,
        };
        assert_eq!(terminate.encode(), (TERMINATE, [20, 21, 0, 0]));
        let thread = Call::Thread {
            entry: 22,
            stack: 23,
            argument: 24,
        };
        assert_eq!(thread.encode(), (THREAD, [22, 23, 24, 0]));
        assert_eq!(Call::EndThread(25).encode(), (END_THREAD, [25, 0, 0, 0]));
        let remove = Call::Remove { len: 26, at: 27 };
        assert_eq!(remove.encode(), (REMOVE, [26, 27, 0, 0]));
        let directory = Call::MakeDirectory { len: 28, at: 29 };
        assert_eq!(directory.encode(), (MAKE_DIRECTORY, [28, 29, 0, 0]));
        assert_eq!(Call::Unknown(40).encode(), (40, [0; 4]));
    }

    #[test]
    fn answers_are_errors_only_at_the_top() {
        for (i, error) in ERRORS.into_iter().enumerate() {
            assert_eq!(answer(u64::MAX - i as u64), Err(error));
        }
        assert_eq!(answer(u64::MAX), Err(Error::NoSuchCall));
        assert_eq!(answer(NOT_A_PROGRAM), Err(Error::NotAProgram));
        assert_eq!(answer(BAD_ARGUMENT), Err(Error::BadArgument));
        assert_eq!(answer(NOT_FOUND), Err(Error::NotFound));
        assert_eq!(answer(NOT_A_FILE), Err(Error::NotAFile));
        assert_eq!(answer(UNREADABLE), Err(Error::Unreadable));
        assert_eq!(answer(NOT_A_DIRECTORY), Err(Error::NotADirectory));
        assert_eq!(answer(UNWRITABLE), Err(Error::Unwritable));
        assert_eq!(answer(IN_USE), Err(Error::InUse));
        assert_eq!(answer(INSIDE), Err(Error::Inside));
        assert_eq!(answer(NOT_SAME_VOLUME), Err(Error::NotSameVolume));
        assert_eq!(
            answer(NOT_SAME_VOLUME - 1),
            Ok(NOT_SAME_VOLUME - 1),
            "just below them"
        );
        assert_eq!(answer(0), Ok(0));
        assert_eq!(answer(0x4_0003), Ok(0x4_0003), "a handle");
        assert_eq!(answer(USER_END), Ok(USER_END));
        assert_eq!(
            answer(0x10_0000_0000),
            Ok(0x10_0000_0000),
            "where MAP puts memory"
        );
        assert_eq!(
            answer(USER_END - 4096),
            Ok(USER_END - 4096),
            "the last page there is"
        );
    }

    #[test]
    fn each_error_has_the_code_the_kernel_answers_with() {
        let codes = [
            (Error::NoSuchCall, NO_SUCH_CALL),
            (Error::BadAddress, BAD_ADDRESS),
            (Error::TooLong, TOO_LONG),
            (Error::BadHandle, BAD_HANDLE),
            (Error::Denied, DENIED),
            (Error::Full, FULL),
            (Error::Empty, EMPTY),
            (Error::Closed, CLOSED),
            (Error::TooSmall, TOO_SMALL),
            (Error::NoRoom, NO_ROOM),
            (Error::BadSize, BAD_SIZE),
            (Error::NoSuchProgram, NO_SUCH_PROGRAM),
            (Error::NotAProgram, NOT_A_PROGRAM),
            (Error::BadArgument, BAD_ARGUMENT),
            (Error::NotFound, NOT_FOUND),
            (Error::NotAFile, NOT_A_FILE),
            (Error::Unreadable, UNREADABLE),
            (Error::NotADirectory, NOT_A_DIRECTORY),
            (Error::Exists, EXISTS),
            (Error::ReadOnly, READ_ONLY),
            (Error::DiskFull, DISK_FULL),
            (Error::NotEmpty, NOT_EMPTY),
            (Error::Unwritable, UNWRITABLE),
            (Error::InUse, IN_USE),
            (Error::Inside, INSIDE),
            (Error::NotSameVolume, NOT_SAME_VOLUME),
        ];
        for (error, code) in codes {
            assert_eq!(error.code(), code, "{error:?}");
            assert_eq!(answer(code), Err(error));
        }
    }

    #[test]
    fn only_the_lower_half_is_the_users() {
        assert!(in_user_half(0x40_0000, 4096));
        assert!(in_user_half(0, 0));
        assert!(in_user_half(USER_END - 1, 1), "the last byte of the half");
        assert!(in_user_half(USER_END, 0), "nothing at the end");
        assert!(!in_user_half(USER_END - 1, 2), "one byte past the half");
        assert!(!in_user_half(USER_END, 1));
        assert!(
            !in_user_half(0xffff_8000_0000_0000, 8),
            "the kernel's direct map"
        );
        assert!(!in_user_half(0x40_0000, u64::MAX), "wrapping around");
        assert!(!in_user_half(u64::MAX, 1));
    }
}
