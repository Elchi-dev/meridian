//! dd: copies a file a block at a time, converting it on the way, as
//! POSIX's dd does: `if=` and `of=` name the files, `ibs=`, `obs=` and
//! `bs=` the sizes of the blocks read and written, `skip=` and `seek=`
//! the blocks to pass over in each, `count=` how many to read, and
//! `conv=` what to do with them: `ascii`, `ebcdic` and `ibm` between
//! ASCII and EBCDIC, `block` and `unblock` between lines and records of
//! `cbs=` bytes, `lcase` and `ucase`, `swab`, `sync`, `noerror` and
//! `notrunc`. At the end it says how many whole and partial blocks it
//! read and wrote. It ends with 0, or 1 when something went wrong.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write as _;

use program::abi::file::{CREATE_ALWAYS, OPEN_ALWAYS, READ, WRITE};
use program::{Error, Handle, text};

program::main!(dd);

/// ASCII's bytes as EBCDIC has them, and as IBM's EBCDIC has them, the
/// tables POSIX gives; reading back from EBCDIC is the first one turned
/// round.
const EBCDIC: [u8; 256] = [
    0o000, 0o001, 0o002, 0o003, 0o067, 0o055, 0o056, 0o057, 0o026, 0o005, 0o045, 0o013, 0o014,
    0o015, 0o016, 0o017, 0o020, 0o021, 0o022, 0o023, 0o074, 0o075, 0o062, 0o046, 0o030, 0o031,
    0o077, 0o047, 0o034, 0o035, 0o036, 0o037, 0o100, 0o132, 0o177, 0o173, 0o133, 0o154, 0o120,
    0o175, 0o115, 0o135, 0o134, 0o116, 0o153, 0o140, 0o113, 0o141, 0o360, 0o361, 0o362, 0o363,
    0o364, 0o365, 0o366, 0o367, 0o370, 0o371, 0o172, 0o136, 0o114, 0o176, 0o156, 0o157, 0o174,
    0o301, 0o302, 0o303, 0o304, 0o305, 0o306, 0o307, 0o310, 0o311, 0o321, 0o322, 0o323, 0o324,
    0o325, 0o326, 0o327, 0o330, 0o331, 0o342, 0o343, 0o344, 0o345, 0o346, 0o347, 0o350, 0o351,
    0o255, 0o340, 0o275, 0o232, 0o155, 0o171, 0o201, 0o202, 0o203, 0o204, 0o205, 0o206, 0o207,
    0o210, 0o211, 0o221, 0o222, 0o223, 0o224, 0o225, 0o226, 0o227, 0o230, 0o231, 0o242, 0o243,
    0o244, 0o245, 0o246, 0o247, 0o250, 0o251, 0o300, 0o117, 0o320, 0o137, 0o007, 0o040, 0o041,
    0o042, 0o043, 0o044, 0o025, 0o006, 0o027, 0o050, 0o051, 0o052, 0o053, 0o054, 0o011, 0o012,
    0o033, 0o060, 0o061, 0o032, 0o063, 0o064, 0o065, 0o066, 0o010, 0o070, 0o071, 0o072, 0o073,
    0o004, 0o024, 0o076, 0o341, 0o101, 0o102, 0o103, 0o104, 0o105, 0o106, 0o107, 0o110, 0o111,
    0o121, 0o122, 0o123, 0o124, 0o125, 0o126, 0o127, 0o130, 0o131, 0o142, 0o143, 0o144, 0o145,
    0o146, 0o147, 0o150, 0o151, 0o160, 0o161, 0o162, 0o163, 0o164, 0o165, 0o166, 0o167, 0o170,
    0o200, 0o212, 0o213, 0o214, 0o215, 0o216, 0o217, 0o220, 0o152, 0o233, 0o234, 0o235, 0o236,
    0o237, 0o240, 0o252, 0o253, 0o254, 0o112, 0o256, 0o257, 0o260, 0o261, 0o262, 0o263, 0o264,
    0o265, 0o266, 0o267, 0o270, 0o271, 0o272, 0o273, 0o274, 0o241, 0o276, 0o277, 0o312, 0o313,
    0o314, 0o315, 0o316, 0o317, 0o332, 0o333, 0o334, 0o335, 0o336, 0o337, 0o352, 0o353, 0o354,
    0o355, 0o356, 0o357, 0o372, 0o373, 0o374, 0o375, 0o376, 0o377,
];
const IBM: [u8; 256] = [
    0o000, 0o001, 0o002, 0o003, 0o067, 0o055, 0o056, 0o057, 0o026, 0o005, 0o045, 0o013, 0o014,
    0o015, 0o016, 0o017, 0o020, 0o021, 0o022, 0o023, 0o074, 0o075, 0o062, 0o046, 0o030, 0o031,
    0o077, 0o047, 0o034, 0o035, 0o036, 0o037, 0o100, 0o132, 0o177, 0o173, 0o133, 0o154, 0o120,
    0o175, 0o115, 0o135, 0o134, 0o116, 0o153, 0o140, 0o113, 0o141, 0o360, 0o361, 0o362, 0o363,
    0o364, 0o365, 0o366, 0o367, 0o370, 0o371, 0o172, 0o136, 0o114, 0o176, 0o156, 0o157, 0o174,
    0o301, 0o302, 0o303, 0o304, 0o305, 0o306, 0o307, 0o310, 0o311, 0o321, 0o322, 0o323, 0o324,
    0o325, 0o326, 0o327, 0o330, 0o331, 0o342, 0o343, 0o344, 0o345, 0o346, 0o347, 0o350, 0o351,
    0o255, 0o340, 0o275, 0o137, 0o155, 0o171, 0o201, 0o202, 0o203, 0o204, 0o205, 0o206, 0o207,
    0o210, 0o211, 0o221, 0o222, 0o223, 0o224, 0o225, 0o226, 0o227, 0o230, 0o231, 0o242, 0o243,
    0o244, 0o245, 0o246, 0o247, 0o250, 0o251, 0o300, 0o117, 0o320, 0o241, 0o007, 0o040, 0o041,
    0o042, 0o043, 0o044, 0o025, 0o006, 0o027, 0o050, 0o051, 0o052, 0o053, 0o054, 0o011, 0o012,
    0o033, 0o060, 0o061, 0o032, 0o063, 0o064, 0o065, 0o066, 0o010, 0o070, 0o071, 0o072, 0o073,
    0o004, 0o024, 0o076, 0o341, 0o101, 0o102, 0o103, 0o104, 0o105, 0o106, 0o107, 0o110, 0o111,
    0o121, 0o122, 0o123, 0o124, 0o125, 0o126, 0o127, 0o130, 0o131, 0o142, 0o143, 0o144, 0o145,
    0o146, 0o147, 0o150, 0o151, 0o160, 0o161, 0o162, 0o163, 0o164, 0o165, 0o166, 0o167, 0o170,
    0o200, 0o212, 0o213, 0o214, 0o215, 0o216, 0o217, 0o220, 0o232, 0o233, 0o234, 0o235, 0o236,
    0o237, 0o240, 0o252, 0o253, 0o254, 0o255, 0o256, 0o257, 0o260, 0o261, 0o262, 0o263, 0o264,
    0o265, 0o266, 0o267, 0o270, 0o271, 0o272, 0o273, 0o274, 0o275, 0o276, 0o277, 0o312, 0o313,
    0o314, 0o315, 0o316, 0o317, 0o332, 0o333, 0o334, 0o335, 0o336, 0o337, 0o352, 0o353, 0o354,
    0o355, 0o356, 0o357, 0o372, 0o373, 0o374, 0o375, 0o376, 0o377,
];

/// Between which codes the bytes are turned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Code {
    Ascii,
    Ebcdic,
    Ibm,
}

/// Between which shapes of records the text is turned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Block,
    Unblock,
}

/// What dd keeps: going after the input could not be read, with
/// `noerror`, and what the output held past what it writes, with
/// `notrunc`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Keeps {
    going: bool,
    the_rest: bool,
}

/// What `conv=` asks for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Conv {
    code: Option<Code>,
    shape: Option<Shape>,
    /// Small letters with `false`, capitals with `true`.
    case: Option<bool>,
    swab: bool,
    sync: bool,
    keeps: Keeps,
}

impl Conv {
    /// Whether anything is done to the bytes between reading and writing
    /// them.
    fn converts(self) -> bool {
        self.code.is_some() || self.shape.is_some() || self.case.is_some() || self.swab
    }
}

/// What dd's operands ask for.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'w> {
    input: Option<&'w str>,
    output: Option<&'w str>,
    ibs: usize,
    obs: usize,
    /// Whether `bs=` gave both sizes, which keeps each block read whole.
    bs: bool,
    cbs: usize,
    skip: u64,
    seek: u64,
    count: Option<u64>,
    conv: Conv,
}

/// A size as `bs=` and its kin write one: numbers of bytes, each times
/// 1024 with `k` or 512 with `b` after it, multiplied where `x` parts
/// them.
fn size(written: &str) -> Option<usize> {
    written.split('x').try_fold(1usize, |all, part| {
        let (digits, times) = match part.as_bytes().last()? {
            b'k' => (&part[..part.len() - 1], 1024),
            b'b' => (&part[..part.len() - 1], 512),
            _ => (part, 1),
        };
        // A sign is no digit, though parse would take one.
        if !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        digits
            .parse::<usize>()
            .ok()?
            .checked_mul(times)?
            .checked_mul(all)
    })
}

/// A count of blocks, as `skip=`, `seek=` and `count=` take one.
fn blocks(written: &str) -> Option<u64> {
    if !written.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    written.parse().ok()
}

/// What `conv=` with `written` asks for, or why it cannot be taken.
fn conv(written: &str) -> Result<Conv, String> {
    let mut conv = Conv::default();
    for value in written.split(',') {
        let both = |what: &str| format!("dd: conv= takes one of {what}\n");
        match value {
            "ascii" | "ebcdic" | "ibm" if conv.code.is_some() => {
                return Err(both("ascii, ebcdic and ibm"));
            }
            "ascii" => conv.code = Some(Code::Ascii),
            "ebcdic" => conv.code = Some(Code::Ebcdic),
            "ibm" => conv.code = Some(Code::Ibm),
            "block" | "unblock" if conv.shape.is_some() => return Err(both("block and unblock")),
            "block" => conv.shape = Some(Shape::Block),
            "unblock" => conv.shape = Some(Shape::Unblock),
            "lcase" | "ucase" if conv.case.is_some() => return Err(both("lcase and ucase")),
            "lcase" => conv.case = Some(false),
            "ucase" => conv.case = Some(true),
            "swab" => conv.swab = true,
            "noerror" => conv.keeps.going = true,
            "notrunc" => conv.keeps.the_rest = true,
            "sync" => conv.sync = true,
            _ => return Err(format!("dd: conv={value} is no conversion\n")),
        }
    }
    Ok(conv)
}

/// What dd's operands ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut asked = Asked {
        input: None,
        output: None,
        ibs: 512,
        obs: 512,
        bs: false,
        cbs: 0,
        skip: 0,
        seek: 0,
        count: None,
        conv: Conv::default(),
    };
    let mut bs = None;
    for word in arguments {
        let Some((name, value)) = word.split_once('=') else {
            return Err(format!("dd: {word} is no operand\n"));
        };
        let wrong = || format!("dd: {name}= wants a size, not {value}\n");
        let count = || format!("dd: {name}= wants a number of blocks, not {value}\n");
        match name {
            "if" => asked.input = Some(value),
            "of" => asked.output = Some(value),
            "ibs" | "obs" | "bs" | "cbs" => {
                let bytes = size(value).filter(|&n| n > 0).ok_or_else(wrong)?;
                match name {
                    "ibs" => asked.ibs = bytes,
                    "obs" => asked.obs = bytes,
                    "bs" => bs = Some(bytes),
                    _ => asked.cbs = bytes,
                }
            }
            "skip" => asked.skip = blocks(value).ok_or_else(count)?,
            "seek" => asked.seek = blocks(value).ok_or_else(count)?,
            "count" => asked.count = Some(blocks(value).ok_or_else(count)?),
            "conv" => {
                let more = conv(value)?;
                asked.conv = merged(asked.conv, more)?;
            }
            _ => return Err(format!("dd: {name}= is no operand\n")),
        }
    }
    if let Some(bytes) = bs {
        (asked.ibs, asked.obs, asked.bs) = (bytes, bytes, true);
    }
    let shaped = asked.conv.shape.is_some() || asked.conv.code.is_some();
    if shaped && asked.cbs == 0 {
        return Err(String::from(
            "dd: block, unblock, ascii, ebcdic and ibm want cbs=\n",
        ));
    }
    Ok(asked)
}

/// The one of `x` and `y` given, or why both cannot be.
fn either<T>(x: Option<T>, y: Option<T>, what: &str) -> Result<Option<T>, String> {
    match (x, y) {
        (Some(_), Some(_)) => Err(format!("dd: conv= takes one of {what}\n")),
        (x, y) => Ok(x.or(y)),
    }
}

/// Two `conv=` operands taken together.
fn merged(a: Conv, b: Conv) -> Result<Conv, String> {
    Ok(Conv {
        code: either(a.code, b.code, "ascii, ebcdic and ibm")?,
        shape: either(a.shape, b.shape, "block and unblock")?,
        case: either(a.case, b.case, "lcase and ucase")?,
        swab: a.swab || b.swab,
        sync: a.sync || b.sync,
        keeps: Keeps {
            going: a.keeps.going || b.keeps.going,
            the_rest: a.keeps.the_rest || b.keeps.the_rest,
        },
    })
}

/// How many blocks went whole and how many in part.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Records {
    whole: u64,
    part: u64,
}

impl Records {
    fn count(&mut self, len: usize, size: usize) {
        if len == size {
            self.whole += 1;
        } else {
            self.part += 1;
        }
    }
}

/// What dd says at the end: the blocks read and written, and the lines
/// cut short to fit a record.
fn summary(read: Records, written: Records, truncated: u64) -> String {
    let mut said = format!(
        "{}+{} records in\n{}+{} records out\n",
        read.whole, read.part, written.whole, written.part
    );
    if truncated > 0 {
        let records = if truncated == 1 { "record" } else { "records" };
        let _ = writeln!(said, "{truncated} truncated {records}");
    }
    said
}

/// The bytes on their way from the blocks read to the blocks written,
/// and what has been counted of them.
struct Copy<'a> {
    asked: &'a Asked<'a>,
    /// EBCDIC's bytes as ASCII has them.
    ascii: [u8; 256],
    /// The record `block` is filling, or the bytes `unblock` has of one.
    record: Vec<u8>,
    /// Whether `block` filled the record of the line it is in, and
    /// whether more of the line came, which it cut.
    full: bool,
    cut: bool,
    held: Vec<u8>,
    read: Records,
    written: Records,
    truncated: u64,
}

impl<'a> Copy<'a> {
    fn new(asked: &'a Asked<'a>) -> Copy<'a> {
        let mut ascii = [0; 256];
        for (byte, &ebcdic) in (0..=u8::MAX).zip(&EBCDIC) {
            ascii[usize::from(ebcdic)] = byte;
        }
        Copy {
            asked,
            ascii,
            record: Vec::new(),
            full: false,
            cut: false,
            held: Vec::new(),
            read: Records::default(),
            written: Records::default(),
            truncated: 0,
        }
    }

    /// The byte a padded record is padded with.
    fn pad(&self) -> u8 {
        if self.asked.conv.shape.is_some() || self.asked.conv.code.is_some() {
            b' '
        } else {
            0
        }
    }

    /// Takes a block read, which `write` writes on as blocks of `obs`.
    fn take(
        &mut self,
        mut block: Vec<u8>,
        write: &mut impl FnMut(&[u8]) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let asked = self.asked;
        self.read.count(block.len(), asked.ibs);
        if asked.conv.sync {
            block.resize(asked.ibs, self.pad());
        }
        if asked.bs && !asked.conv.converts() {
            self.written.count(block.len(), asked.obs);
            return write(&block);
        }
        if asked.conv.swab {
            for pair in block.as_chunks_mut::<2>().0 {
                pair.swap(0, 1);
            }
        }
        for byte in block {
            self.convert(byte);
        }
        self.flush(false, write)
    }

    /// Takes one byte through the conversions asked for.
    fn convert(&mut self, mut byte: u8) {
        let conv = self.asked.conv;
        if conv.code == Some(Code::Ascii) {
            byte = self.ascii[usize::from(byte)];
        }
        match conv.case {
            Some(true) => byte = byte.to_ascii_uppercase(),
            Some(false) => byte = byte.to_ascii_lowercase(),
            None => {}
        }
        let block =
            conv.shape == Some(Shape::Block) || matches!(conv.code, Some(Code::Ebcdic | Code::Ibm));
        let unblock = conv.shape == Some(Shape::Unblock) || conv.code == Some(Code::Ascii);
        if block {
            self.block(byte);
        } else if unblock {
            self.record.push(byte);
            if self.record.len() == self.asked.cbs {
                self.unblock();
            }
        } else {
            self.held.push(byte);
        }
    }

    /// Takes a byte into the record `block` fills: a line feed ends it,
    /// padded with spaces, and a line too long is cut.
    fn block(&mut self, byte: u8) {
        if byte == b'\n' {
            if !self.full {
                self.end_record();
            }
            (self.full, self.cut) = (false, false);
            return;
        }
        if self.full {
            self.truncated += u64::from(!self.cut);
            self.cut = true;
            return;
        }
        self.record.push(byte);
        if self.record.len() == self.asked.cbs {
            self.end_record();
            self.full = true;
        }
    }

    /// Pads the record `block` fills, turns it into EBCDIC when asked,
    /// and holds it to be written.
    fn end_record(&mut self) {
        let mut record = core::mem::take(&mut self.record);
        record.resize(self.asked.cbs, b' ');
        let table = match self.asked.conv.code {
            Some(Code::Ebcdic) => Some(&EBCDIC),
            Some(Code::Ibm) => Some(&IBM),
            _ => None,
        };
        if let Some(table) = table {
            for byte in &mut record {
                *byte = table[usize::from(*byte)];
            }
        }
        self.held.extend_from_slice(&record);
    }

    /// Ends the record `unblock` has: its spaces at the end go, and a
    /// line feed follows it.
    fn unblock(&mut self) {
        let record = core::mem::take(&mut self.record);
        let kept = record
            .iter()
            .rposition(|&b| b != b' ')
            .map_or(0, |at| at + 1);
        self.held.extend_from_slice(&record[..kept]);
        self.held.push(b'\n');
    }

    /// Writes the whole blocks of `obs` held, and with `last` what is
    /// left as a block of its own.
    fn flush(
        &mut self,
        last: bool,
        write: &mut impl FnMut(&[u8]) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let obs = self.asked.obs;
        while self.held.len() >= obs {
            let block: Vec<u8> = self.held.drain(..obs).collect();
            self.written.count(block.len(), obs);
            write(&block)?;
        }
        if last && !self.held.is_empty() {
            let block = core::mem::take(&mut self.held);
            self.written.count(block.len(), obs);
            write(&block)?;
        }
        Ok(())
    }

    /// Ends the input: the record in hand ends, and what is held goes out.
    fn finish(&mut self, write: &mut impl FnMut(&[u8]) -> Result<(), Error>) -> Result<(), Error> {
        let conv = self.asked.conv;
        let block =
            conv.shape == Some(Shape::Block) || matches!(conv.code, Some(Code::Ebcdic | Code::Ibm));
        if !self.record.is_empty() {
            if block {
                self.end_record();
            } else {
                self.unblock();
            }
        }
        self.flush(true, write)
    }
}

/// What dd reads from and writes to.
trait Files {
    /// Opens the input, the file at `path` or the standard input, from
    /// its byte `at` on where it can go there at once.
    ///
    /// # Errors
    ///
    /// When it does not open.
    fn open_input(&mut self, path: Option<&str>, at: u64) -> Result<bool, Error>;

    /// The next piece of the input, of `room` bytes at most; none at its
    /// end.
    ///
    /// # Errors
    ///
    /// When it cannot be read.
    fn read(&mut self, room: usize) -> Result<Vec<u8>, Error>;

    /// Opens the output, the file at `path` or the standard output: with
    /// `keep` as it is, else emptied, answering the first `before` bytes
    /// it held.
    ///
    /// # Errors
    ///
    /// When it does not open.
    fn open_output(
        &mut self,
        path: Option<&str>,
        keep: bool,
        before: u64,
    ) -> Result<Vec<u8>, Error>;

    /// Writes `bytes` into the output from its byte `at` on, the standard
    /// output after what it was given.
    ///
    /// # Errors
    ///
    /// When they cannot be written.
    fn write(&mut self, at: u64, bytes: &[u8]) -> Result<(), Error>;

    /// Says `text` on the standard error.
    fn say(&mut self, text: &str);
}

/// Opens the input and goes past the blocks `skip=` names, saying what
/// went wrong; whether that went.
fn start_input(files: &mut impl Files, asked: &Asked, name: &str) -> bool {
    let skipped = u64::try_from(asked.ibs).map_or(u64::MAX, |ibs| ibs.saturating_mul(asked.skip));
    let went_there = match files.open_input(asked.input, skipped) {
        Ok(went) => went,
        Err(e) => {
            files.say(&text::trouble("dd", name, e));
            return false;
        }
    };
    if !went_there {
        // An input that cannot go there at once is read up to it.
        for _ in 0..asked.skip {
            match files.read(asked.ibs) {
                Ok(piece) if !piece.is_empty() => {}
                Ok(_) => {
                    files.say(&format!(
                        "dd: {name}: it ends before the blocks skip= passes\n"
                    ));
                    break;
                }
                Err(e) => {
                    files.say(&text::trouble("dd", name, e));
                    return false;
                }
            }
        }
    }
    true
}

/// Copies as `asked` says, and answers the status dd ends with.
fn copy(files: &mut impl Files, asked: &Asked) -> u64 {
    let name = asked.input.unwrap_or("-");
    if !start_input(files, asked, name) {
        return 1;
    }
    let at = u64::try_from(asked.obs).map_or(u64::MAX, |obs| obs.saturating_mul(asked.seek));
    if asked.output.is_none() && asked.seek > 0 {
        files.say("dd: seek= wants of=\n");
        return 1;
    }
    let out = asked.output.unwrap_or("-");
    let keep = asked.conv.keeps.the_rest;
    // Without notrunc what lies before the blocks sought past stays, and
    // nothing after them.
    let opened = files
        .open_output(asked.output, keep, at)
        .and_then(|mut before| {
            if keep || at == 0 {
                return Ok(());
            }
            before.resize(usize::try_from(at).unwrap_or(usize::MAX), 0);
            files.write(0, &before)
        });
    if let Err(e) = opened {
        files.say(&text::trouble("dd", out, e));
        return 1;
    }
    let mut at = at;
    let mut put = |files: &mut _, writes: &mut Vec<Vec<u8>>| -> Result<(), Error> {
        for bytes in writes.drain(..) {
            Files::write(files, at, &bytes)?;
            at += bytes.len() as u64;
        }
        Ok(())
    };
    let mut copy = Copy::new(asked);
    let mut trouble = None;
    let mut blocks = 0;
    let mut writes = Vec::new();
    loop {
        if asked.count.is_some_and(|count| blocks >= count) {
            break;
        }
        let block = match files.read(asked.ibs) {
            Ok(block) if block.is_empty() => break,
            Ok(block) => block,
            Err(e) => {
                files.say(&text::trouble("dd", name, e));
                // A stream cannot go past what it could not read.
                if !asked.conv.keeps.going || asked.input.is_none() {
                    trouble = Some(());
                    break;
                }
                files.say(&summary(copy.read, copy.written, copy.truncated));
                if !asked.conv.sync {
                    blocks += 1;
                    continue;
                }
                Vec::new()
            }
        };
        blocks += 1;
        let taken = copy.take(block, &mut |bytes| {
            writes.push(bytes.to_vec());
            Ok(())
        });
        if let Err(e) = taken.and_then(|()| put(files, &mut writes)) {
            files.say(&text::trouble("dd", out, e));
            files.say(&summary(copy.read, copy.written, copy.truncated));
            return 1;
        }
    }
    let finished = copy.finish(&mut |bytes| {
        writes.push(bytes.to_vec());
        Ok(())
    });
    let written = finished.and_then(|()| put(files, &mut writes));
    if let Err(e) = written {
        files.say(&text::trouble("dd", out, e));
        trouble = Some(());
    }
    files.say(&summary(copy.read, copy.written, copy.truncated));
    u64::from(trouble.is_some())
}

/// Meridian's files, and the program's own streams.
#[derive(Default)]
struct System {
    input: Option<(Handle, u64)>,
    /// What the standard input gave beyond the last read.
    left: Vec<u8>,
    output: Option<Handle>,
}

impl Files for System {
    fn open_input(&mut self, path: Option<&str>, at: u64) -> Result<bool, Error> {
        let Some(path) = path else {
            return Ok(false);
        };
        self.input = Some((program::open(path, READ)?, at));
        Ok(true)
    }

    fn read(&mut self, room: usize) -> Result<Vec<u8>, Error> {
        let Some((file, at)) = &mut self.input else {
            if self.left.is_empty() {
                let mut piece = [0u8; program::abi::MESSAGE_BYTES];
                let len = program::input(&mut piece)?;
                self.left.extend_from_slice(&piece[..len]);
            }
            let taken = room.min(self.left.len());
            return Ok(self.left.drain(..taken).collect());
        };
        let mut bytes = vec![0; room];
        // A block that cannot be read is passed over all the same.
        let read = program::read(*file, *at, &mut bytes);
        *at += read.as_ref().map_or(room, |&len| len) as u64;
        bytes.truncate(read?);
        Ok(bytes)
    }

    fn open_output(
        &mut self,
        path: Option<&str>,
        keep: bool,
        before: u64,
    ) -> Result<Vec<u8>, Error> {
        let Some(path) = path else {
            return Ok(Vec::new());
        };
        let mut held = Vec::new();
        if !keep {
            let _ = text::pieces(Some(path), |piece| {
                held.extend_from_slice(piece);
                (held.len() as u64) < before
            });
            held.truncate(usize::try_from(before).unwrap_or(usize::MAX));
        }
        let how = if keep { OPEN_ALWAYS } else { CREATE_ALWAYS };
        self.output = Some(program::open_with(path, WRITE, how)?);
        Ok(held)
    }

    fn write(&mut self, at: u64, bytes: &[u8]) -> Result<(), Error> {
        match self.output {
            Some(file) => program::write_file(file, at, bytes),
            None => program::write_bytes(bytes),
        }
    }

    fn say(&mut self, text: &str) {
        let _ = program::write_error(text);
    }
}

#[cfg_attr(test, allow(dead_code))]
fn dd(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
    let mut system = System::default();
    let status = copy(&mut system, &asked);
    let input = system.input.map(|(file, _)| file);
    for file in [input, system.output].into_iter().flatten() {
        let _ = program::close(file);
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::FileExt as _;

    /// Where `path` is under `root`, a path from the top taken from
    /// `root`. One that climbs out of it stops the test, so that no
    /// mistake in the program reaches the host's own files.
    fn confined(root: &std::path::Path, path: &str) -> std::path::PathBuf {
        let mut depth = 0usize;
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    depth = depth
                        .checked_sub(1)
                        .unwrap_or_else(|| panic!("{path} climbs out of the test's directory"));
                }
                _ => depth += 1,
            }
        }
        root.join(path.trim_start_matches('/'))
    }

    fn words(text: &str) -> Vec<String> {
        text.split(' ')
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    /// Files among the host's temporary files, gone once dropped; the
    /// standard input, given out at most `piece` bytes a read as a pipe
    /// gives it; and what dd wrote on its standard output and said.
    struct Host {
        root: std::path::PathBuf,
        stdin: Vec<u8>,
        piece: usize,
        /// A file the standard input reads instead, as `<` makes it.
        stdin_file: Option<std::fs::File>,
        input: Option<(std::fs::File, u64)>,
        output: Option<std::fs::File>,
        written: Vec<u8>,
        said: String,
    }

    impl Host {
        fn new(name: &str) -> Host {
            let root =
                std::env::temp_dir().join(format!("meridian-dd-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Host {
                root,
                stdin: Vec::new(),
                piece: usize::MAX,
                stdin_file: None,
                input: None,
                output: None,
                written: Vec::new(),
                said: String::new(),
            }
        }

        fn holds(&self, name: &str) -> Vec<u8> {
            std::fs::read(confined(&self.root, name)).unwrap()
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn error(e: &std::io::Error) -> Error {
        match e.kind() {
            std::io::ErrorKind::NotFound => Error::NotFound,
            std::io::ErrorKind::IsADirectory => Error::NotAFile,
            _ => Error::Unreadable,
        }
    }

    impl Files for Host {
        fn open_input(&mut self, path: Option<&str>, at: u64) -> Result<bool, Error> {
            let Some(path) = path else {
                return Ok(false);
            };
            let file = std::fs::File::open(confined(&self.root, path)).map_err(|e| error(&e))?;
            self.input = Some((file, at));
            Ok(true)
        }

        fn read(&mut self, room: usize) -> Result<Vec<u8>, Error> {
            let Some((file, at)) = &mut self.input else {
                if let Some(file) = &mut self.stdin_file {
                    let mut bytes = vec![0; room];
                    let len = std::io::Read::read(file, &mut bytes).map_err(|e| error(&e))?;
                    bytes.truncate(len);
                    return Ok(bytes);
                }
                let taken = room.min(self.piece).min(self.stdin.len());
                return Ok(self.stdin.drain(..taken).collect());
            };
            let mut bytes = vec![0; room];
            let read = file.read_at(&mut bytes, *at);
            *at += read.as_ref().map_or(room, |&len| len) as u64;
            bytes.truncate(read.map_err(|e| error(&e))?);
            Ok(bytes)
        }

        fn open_output(
            &mut self,
            path: Option<&str>,
            keep: bool,
            before: u64,
        ) -> Result<Vec<u8>, Error> {
            let Some(path) = path else {
                return Ok(Vec::new());
            };
            let at = confined(&self.root, path);
            let mut held = std::fs::read(&at).unwrap_or_default();
            held.truncate(usize::try_from(before).unwrap());
            let file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(!keep)
                .open(at)
                .map_err(|e| error(&e))?;
            self.output = Some(file);
            Ok(if keep { Vec::new() } else { held })
        }

        fn write(&mut self, at: u64, bytes: &[u8]) -> Result<(), Error> {
            if let Some(file) = &self.output {
                return file.write_all_at(bytes, at).map_err(|e| error(&e));
            }
            self.written.extend_from_slice(bytes);
            Ok(())
        }

        fn say(&mut self, text: &str) {
            self.said.push_str(text);
        }
    }

    /// How dd with the operands `text` ran on `host`, its standard input
    /// `input`: its status, what it wrote and what it said.
    fn ran(host: &mut Host, text: &str, input: &[u8]) -> (u64, Vec<u8>, String) {
        host.stdin = input.to_vec();
        let given = words(text);
        let status = copy(host, &asked(&given).unwrap());
        host.input = None;
        host.output = None;
        (
            status,
            core::mem::take(&mut host.written),
            core::mem::take(&mut host.said),
        )
    }

    fn thousand() -> Vec<u8> {
        (0..1000u32)
            .map(|i| u8::try_from(i * 7 % 251).unwrap())
            .collect()
    }

    fn said(text: &str) -> String {
        String::from(text)
    }

    #[test]
    fn blocks_are_read_and_written_as_gnu_s_dd_counts_them() {
        let mut host = Host::new("blocks");
        let all = thousand();
        assert_eq!(
            ran(&mut host, "", &all),
            (0, all.clone(), said("1+1 records in\n1+1 records out\n"))
        );
        assert_eq!(
            ran(&mut host, "bs=100 count=3", &all),
            (
                0,
                all[..300].to_vec(),
                said("3+0 records in\n3+0 records out\n")
            )
        );
        assert_eq!(
            ran(&mut host, "ibs=100 obs=300", &all),
            (0, all.clone(), said("10+0 records in\n3+1 records out\n"))
        );
        assert_eq!(
            ran(&mut host, "bs=100 skip=8", &all),
            (
                0,
                all[800..].to_vec(),
                said("2+0 records in\n2+0 records out\n")
            )
        );
        assert_eq!(
            ran(&mut host, "bs=2 count=0", b"abc"),
            (0, Vec::new(), said("0+0 records in\n0+0 records out\n"))
        );
        assert_eq!(
            ran(&mut host, "bs=1kx2", &[b'z'; 3000]).2,
            "1+1 records in\n1+1 records out\n"
        );
    }

    #[test]
    fn short_reads_stay_short_blocks_unless_sync_pads_them() {
        let mut host = Host::new("short");
        host.piece = 30;
        let x = [b'x'; 100];
        assert_eq!(
            ran(&mut host, "bs=64", &x),
            (0, x.to_vec(), said("0+4 records in\n0+4 records out\n")),
            "each piece of a pipe is a block of its own"
        );
        host.piece = usize::MAX;
        let mut padded = x.to_vec();
        padded.resize(128, 0);
        assert_eq!(
            ran(&mut host, "bs=64 conv=sync", &x),
            (0, padded.clone(), said("1+1 records in\n2+0 records out\n"))
        );
        assert_eq!(
            ran(&mut host, "ibs=64 conv=sync", &x),
            (0, padded, said("1+1 records in\n0+1 records out\n"))
        );
        host.piece = 3;
        assert_eq!(
            ran(&mut host, "ibs=2 obs=5", b"abcdefg").1,
            b"abcdefg",
            "a read is cut to its block"
        );
    }

    #[test]
    fn letters_bytes_and_records_are_converted_as_gnu_s_dd_does() {
        let mut host = Host::new("conv");
        let case = |host: &mut Host, text, input: &[u8]| ran(host, text, input).1;
        assert_eq!(
            case(&mut host, "conv=ucase", b"Hello, World 1\n"),
            b"HELLO, WORLD 1\n"
        );
        assert_eq!(
            case(&mut host, "conv=lcase", b"Hello, World 1\n"),
            b"hello, world 1\n"
        );
        assert_eq!(case(&mut host, "conv=swab", b"abcde"), b"badce");
        assert_eq!(
            case(&mut host, "conv=unblock cbs=4", b"ab  cd  efgh"),
            b"ab\ncd\nefgh\n"
        );
        assert_eq!(
            case(&mut host, "conv=unblock cbs=4", b"ab  cd"),
            b"ab\ncd\n"
        );
        assert_eq!(
            case(&mut host, "conv=ebcdic cbs=4", b"AB\nc\n"),
            b"\xc1\xc2@@\x83@@@"
        );
        assert_eq!(case(&mut host, "conv=ibm cbs=4", b"^~\n"), b"_\xa1@@");
        assert_eq!(
            case(&mut host, "conv=ascii cbs=4", b"\xc1\xc2@@\x83@@@"),
            b"AB\nc\n"
        );
        assert_eq!(case(&mut host, "conv=block,ucase cbs=4", b"ab\n"), b"AB  ");
        assert_eq!(
            ran(&mut host, "conv=block cbs=8", b"ab\ncdefghijklm\nx"),
            (
                0,
                b"ab      cdefghijx       ".to_vec(),
                said("0+1 records in\n0+1 records out\n1 truncated record\n")
            )
        );
        assert_eq!(
            ran(
                &mut host,
                "conv=block cbs=8",
                b"ab\ncdefghijklm\nnop\nqrstuvwxyz\n"
            )
            .2,
            "0+1 records in\n0+1 records out\n2 truncated records\n"
        );
        assert_eq!(
            ran(&mut host, "conv=block cbs=4", b"abcd\nefgh"),
            (
                0,
                b"abcdefgh".to_vec(),
                said("0+1 records in\n0+1 records out\n")
            ),
            "a line that fills its record is no truncated one"
        );
        assert_eq!(
            ran(&mut host, "conv=block cbs=2", b"abc\nd"),
            (
                0,
                b"abd ".to_vec(),
                said("0+1 records in\n0+1 records out\n1 truncated record\n")
            ),
            "the first line can be the one too long"
        );
        host.piece = 3;
        assert_eq!(
            case(&mut host, "conv=block cbs=4 ibs=3", b"ab\ncdefg\nh"),
            b"ab  cdefh   ",
            "records go on across blocks"
        );
    }

    #[test]
    fn the_ebcdic_tables_are_turned_round_one_for_one() {
        let mut host = Host::new("tables");
        let all: Vec<u8> = (0..=u8::MAX).collect();
        let there: Vec<u8> = all.iter().map(|&b| EBCDIC[usize::from(b)]).collect();
        let mut seen = there.clone();
        seen.sort_unstable();
        assert_eq!(seen, all, "every byte once");
        let ascii = Asked {
            conv: Conv {
                code: Some(Code::Ascii),
                ..Conv::default()
            },
            ..asked(&[]).unwrap()
        };
        let copy = Copy::new(&ascii);
        assert!(
            all.iter()
                .all(|&b| copy.ascii[usize::from(EBCDIC[usize::from(b)])] == b)
        );
        let differ: Vec<usize> = (0..256).filter(|&i| EBCDIC[i] != IBM[i]).collect();
        assert_eq!(
            differ,
            [94, 126, 203, 213, 229],
            "IBM's differs where POSIX marks it"
        );
        assert_eq!(ran(&mut host, "conv=ebcdic cbs=1", b"A").1, [0xc1]);
    }

    #[test]
    fn the_output_keeps_what_lies_before_and_with_notrunc_after() {
        // As GNU's dd leaves the file.
        let mut host = Host::new("seek");
        let before = b"0123456789abcdefghijABCDEFGHIJ";
        for (text, wanted) in [
            ("bs=10 seek=2 of=out", &b"0123456789abcdefghijNEW"[..]),
            (
                "bs=10 seek=2 of=out conv=notrunc",
                b"0123456789abcdefghijNEWDEFGHIJ",
            ),
            ("bs=10 of=out", b"NEW"),
            (
                "bs=10 of=out conv=notrunc",
                b"NEW3456789abcdefghijABCDEFGHIJ",
            ),
        ] {
            std::fs::write(host.root.join("out"), before).unwrap();
            assert_eq!(ran(&mut host, text, b"NEW").0, 0);
            assert_eq!(host.holds("out"), wanted, "{text}");
        }
        std::fs::write(host.root.join("out"), b"0123456789").unwrap();
        ran(&mut host, "bs=10 seek=5 of=out", b"NEW");
        let mut gap = b"0123456789".to_vec();
        gap.resize(50, 0);
        gap.extend_from_slice(b"NEW");
        assert_eq!(host.holds("out"), gap, "what was not there is zeros");
        assert_eq!(
            ran(&mut host, "bs=10 seek=1", b"x"),
            (1, Vec::new(), said("dd: seek= wants of=\n"))
        );
    }

    #[test]
    fn a_file_is_read_from_where_skip_says_and_errors_are_said() {
        let mut host = Host::new("files");
        std::fs::write(host.root.join("in"), thousand()).unwrap();
        assert_eq!(ran(&mut host, "if=in bs=100 skip=9 of=o", b"").0, 0);
        assert_eq!(host.holds("o"), thousand()[900..]);
        assert_eq!(
            ran(&mut host, "if=nope", b""),
            (1, Vec::new(), said("dd: nope: no such file\n"))
        );
        std::fs::create_dir(host.root.join("dir")).unwrap();
        assert_eq!(
            ran(&mut host, "if=dir", b""),
            (
                1,
                Vec::new(),
                said("dd: dir: a directory, not a file\n0+0 records in\n0+0 records out\n")
            )
        );
        let (status, written, words) = ran(&mut host, "if=dir conv=noerror,sync bs=4 count=2", b"");
        assert_eq!(
            (status, written),
            (0, Vec::from([0; 8])),
            "noerror goes on, sync fills what was not read"
        );
        assert_eq!(
            words,
            "dd: dir: a directory, not a file\n0+0 records in\n0+0 records out\n\
             dd: dir: a directory, not a file\n0+1 records in\n1+0 records out\n0+2 records in\n2+0 records out\n"
        );
    }

    #[test]
    fn operands_are_names_with_values() {
        let given = words(
            "if=a of=b ibs=1k obs=2b cbs=3 skip=4 seek=5 count=6 conv=ucase,sync conv=notrunc",
        );
        let taken = asked(&given).unwrap();
        assert_eq!(
            (
                taken.input,
                taken.output,
                taken.ibs,
                taken.obs,
                taken.bs,
                taken.cbs,
                taken.skip,
                taken.seek,
                taken.count
            ),
            (Some("a"), Some("b"), 1024, 1024, false, 3, 4, 5, Some(6))
        );
        assert_eq!(taken.conv.case, Some(true));
        assert!(
            taken.conv.sync
                && taken.conv.keeps.the_rest
                && !taken.conv.keeps.going
                && !taken.conv.swab
        );
        let given = words("ibs=1 obs=2 bs=3");
        let taken = asked(&given).unwrap();
        assert_eq!(
            (taken.ibs, taken.obs, taken.bs),
            (3, 3, true),
            "bs= sets both"
        );
        let given = words("conv=swab,noerror");
        let taken = asked(&given).unwrap();
        assert!(taken.conv.swab && taken.conv.keeps.going);
        assert_eq!(size("2x3x4"), Some(24));
        assert_eq!(size("1kx2b"), Some(1024 * 1024));
        assert_eq!(size(""), None);
        assert_eq!(size("k"), None);
        assert_eq!(size("1x"), None);
        assert_eq!(size("-1"), None);
        assert_eq!(size("+4"), None);
        assert_eq!(size("99999999999999999999k"), None);
        assert_eq!(blocks("7"), Some(7));
        assert_eq!(blocks("7k"), None);
        assert_eq!(blocks("+7"), None);
        assert_eq!(blocks(""), None);
    }

    #[test]
    fn operands_it_cannot_take_are_refused() {
        let refused = |text: &str| asked(&words(text)).err();
        assert_eq!(refused("x"), Some("dd: x is no operand\n".into()));
        assert_eq!(refused("x=1"), Some("dd: x= is no operand\n".into()));
        assert_eq!(
            refused("bs=0"),
            Some("dd: bs= wants a size, not 0\n".into())
        );
        assert_eq!(
            refused("ibs=x"),
            Some("dd: ibs= wants a size, not x\n".into())
        );
        assert_eq!(
            refused("count=-1"),
            Some("dd: count= wants a number of blocks, not -1\n".into())
        );
        assert_eq!(
            refused("conv=xyz"),
            Some("dd: conv=xyz is no conversion\n".into())
        );
        assert_eq!(
            refused("conv=block cbs=1 conv=unblock"),
            Some("dd: conv= takes one of block and unblock\n".into())
        );
        assert_eq!(
            refused("conv=block,unblock cbs=1"),
            Some("dd: conv= takes one of block and unblock\n".into())
        );
        assert_eq!(
            refused("conv=ascii,ebcdic cbs=1"),
            Some("dd: conv= takes one of ascii, ebcdic and ibm\n".into())
        );
        assert_eq!(
            refused("conv=lcase conv=ucase"),
            Some("dd: conv= takes one of lcase and ucase\n".into())
        );
        assert_eq!(
            refused("conv=lcase,ucase"),
            Some("dd: conv= takes one of lcase and ucase\n".into())
        );
        assert_eq!(
            refused("conv=ascii conv=ibm cbs=2"),
            Some("dd: conv= takes one of ascii, ebcdic and ibm\n".into())
        );
        for conv in ["block", "unblock", "ascii", "ebcdic", "ibm"] {
            assert_eq!(
                refused(&format!("conv={conv}")),
                Some("dd: block, unblock, ascii, ebcdic and ibm want cbs=\n".into())
            );
        }
        assert_eq!(
            summary(
                Records { whole: 1, part: 2 },
                Records { whole: 3, part: 4 },
                0
            ),
            "1+2 records in\n3+4 records out\n"
        );
    }

    #[test]
    fn random_copies_through_any_blocks_come_out_whole() {
        let mut seed = 0xbb67_ae85_84ca_a73b_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut host = Host::new("random");
        for _ in 0..300 {
            let len = usize::try_from(next() % 700).unwrap();
            let input: Vec<u8> = (0..len)
                .map(|_| u8::try_from(next() % 256).unwrap())
                .collect();
            let ibs = 1 + next() % 90;
            let obs = 1 + next() % 90;
            host.piece = usize::try_from(1 + next() % 200).unwrap();
            let (status, written, _) = ran(&mut host, &format!("ibs={ibs} obs={obs}"), &input);
            assert_eq!((status, written), (0, input.clone()), "ibs={ibs} obs={obs}");
            // swab swaps within each block, so both runs take the same
            // blocks, of an even size.
            host.piece = usize::MAX;
            let (_, swapped, _) = ran(&mut host, &format!("ibs={} conv=swab", 2 * ibs), &input);
            let (_, back, _) = ran(&mut host, &format!("ibs={} conv=swab", 2 * ibs), &swapped);
            assert_eq!(back, input, "swab twice is no swab");
        }
    }

    #[test]
    fn more_records_as_gnu_s_dd_makes_them() {
        let mut host = Host::new("records");
        let case = |host: &mut Host, text, input: &[u8]| ran(host, text, input);
        host.piece = 5;
        assert_eq!(
            case(&mut host, "ibs=3 conv=sync,block cbs=4", b"ab\ncd"),
            (
                0,
                b"ab  cd  ".to_vec(),
                said("1+1 records in\n0+1 records out\n")
            ),
            "sync pads with spaces where records are made"
        );
        host.piece = usize::MAX;
        assert_eq!(
            case(&mut host, "bs=4 conv=ucase", b"abcdefghij"),
            (
                0,
                b"ABCDEFGHIJ".to_vec(),
                said("2+1 records in\n2+1 records out\n")
            ),
            "with a conversion bs= blocks are gathered as obs= ones"
        );
        assert_eq!(
            case(&mut host, "conv=unblock cbs=3", b"   ab c  ").1,
            b"\nab\nc\n",
            "a record all spaces is an empty line"
        );
        assert_eq!(
            case(&mut host, "conv=unblock cbs=2", b""),
            (0, Vec::new(), said("0+0 records in\n0+0 records out\n"))
        );
        assert_eq!(
            case(&mut host, "conv=block cbs=3", b"\n\nabc\nabcd"),
            (
                0,
                b"      abcabc".to_vec(),
                said("0+1 records in\n0+1 records out\n1 truncated record\n")
            )
        );
        assert_eq!(case(&mut host, "conv=block cbs=1", b"a\n\nbc").1, b"a b");
        assert_eq!(
            case(&mut host, "conv=ascii cbs=2", b"\x40\x40\xc1\x40").1,
            b"\nA\n",
            "turned into ASCII before its spaces go"
        );
        assert_eq!(
            case(&mut host, "conv=ebcdic,lcase cbs=2", b"AB\n").1,
            b"\x81\x82",
            "small letters before EBCDIC"
        );
        assert_eq!(
            case(&mut host, "conv=sync ibs=4 obs=3", b"abcdef"),
            (
                0,
                b"abcdef\0\0".to_vec(),
                said("1+1 records in\n2+1 records out\n")
            )
        );
        assert_eq!(
            case(&mut host, "obs=4", b"abcdefghij").2,
            "0+1 records in\n2+1 records out\n"
        );
        host.piece = 3;
        // GNU's dd swaps across blocks; POSIX leaves the odd last byte of
        // each block as it is.
        assert_eq!(case(&mut host, "conv=swab bs=3", b"abcdefg").1, b"bacedfg");
    }

    #[test]
    fn a_stream_is_read_up_to_where_skip_says() {
        let mut host = Host::new("stream");
        host.piece = 3;
        assert_eq!(
            ran(&mut host, "bs=4 skip=2", b"abcdefghij"),
            (
                0,
                b"ghij".to_vec(),
                said("0+2 records in\n0+2 records out\n")
            ),
            "two reads of a pipe pass, whatever their size"
        );
        host.piece = usize::MAX;
        assert_eq!(ran(&mut host, "bs=4 skip=2", b"abcdefghij").1, b"ij");
        assert_eq!(
            ran(&mut host, "bs=4 skip=5", b"abcdefghij"),
            (
                0,
                Vec::new(),
                said(
                    "dd: -: it ends before the blocks skip= passes\n0+0 records in\n0+0 records out\n"
                )
            )
        );
    }

    #[test]
    fn what_cannot_be_written_or_read_is_said() {
        let mut host = Host::new("trouble");
        std::fs::create_dir(host.root.join("dir")).unwrap();
        assert_eq!(
            ran(&mut host, "of=dir", b"x"),
            (1, Vec::new(), said("dd: dir: a directory, not a file\n"))
        );
        std::fs::create_dir(host.root.join("bad")).unwrap();
        let (status, _, words) = ran(&mut host, "if=bad conv=noerror bs=4 count=2", b"");
        assert_eq!(status, 0, "noerror goes on");
        assert_eq!(
            words,
            "dd: bad: a directory, not a file\n0+0 records in\n0+0 records out\n\
             dd: bad: a directory, not a file\n0+0 records in\n0+0 records out\n0+0 records in\n0+0 records out\n",
            "without sync a block not read is left out"
        );
        assert_eq!(ran(&mut host, "bs=1 seek=3 of=new", b"x").0, 0);
        assert_eq!(
            host.holds("new"),
            b"\0\0\0x",
            "a file not there is made, zeros before"
        );
    }

    #[test]
    fn a_copy_gathers_and_cuts_what_comes_into_blocks_of_obs() {
        let given = words("ibs=4 obs=6");
        let taken = asked(&given).unwrap();
        let mut copy = Copy::new(&taken);
        let mut out: Vec<Vec<u8>> = Vec::new();
        let mut write = |bytes: &[u8]| {
            out.push(bytes.to_vec());
            Ok(())
        };
        copy.take(b"abcd".to_vec(), &mut write).unwrap();
        copy.take(b"ef".to_vec(), &mut write).unwrap();
        copy.take(b"ghij".to_vec(), &mut write).unwrap();
        copy.finish(&mut write).unwrap();
        assert_eq!(out, [b"abcdef".to_vec(), b"ghij".to_vec()]);
        assert_eq!(
            (copy.read, copy.written),
            (Records { whole: 2, part: 1 }, Records { whole: 1, part: 1 })
        );
        let given = words("bs=4 conv=sync");
        let taken = asked(&given).unwrap();
        let mut copy = Copy::new(&taken);
        let mut out = Vec::new();
        copy.take(b"ab".to_vec(), &mut |bytes| {
            out.extend_from_slice(bytes);
            Ok(())
        })
        .unwrap();
        assert_eq!(out, b"ab\0\0", "a block of bs= goes out as it came, padded");
        assert_eq!(copy.pad(), 0);
        let given = words("conv=unblock cbs=2");
        assert_eq!(Copy::new(&asked(&given).unwrap()).pad(), b' ');
    }

    #[test]
    fn what_dd_says_at_its_end() {
        let none = Records::default();
        assert_eq!(
            summary(none, none, 1),
            "0+0 records in\n0+0 records out\n1 truncated record\n"
        );
        assert_eq!(
            summary(none, none, 3),
            "0+0 records in\n0+0 records out\n3 truncated records\n"
        );
        let mut records = Records::default();
        records.count(4, 4);
        records.count(3, 4);
        records.count(5, 4);
        assert_eq!(
            records,
            Records { whole: 1, part: 2 },
            "only a block of the size is whole"
        );
        assert!(!Conv::default().converts());
        assert!(
            Conv {
                swab: true,
                ..Conv::default()
            }
            .converts()
        );
        assert!(
            Conv {
                case: Some(false),
                ..Conv::default()
            }
            .converts()
        );
        assert!(
            Conv {
                shape: Some(Shape::Block),
                ..Conv::default()
            }
            .converts()
        );
        assert!(
            Conv {
                code: Some(Code::Ibm),
                ..Conv::default()
            }
            .converts()
        );
        let kept = Conv {
            sync: true,
            keeps: Keeps {
                going: true,
                the_rest: true,
            },
            ..Conv::default()
        };
        assert!(
            !kept.converts(),
            "sync, noerror and notrunc change no byte read"
        );
    }

    #[test]
    fn a_whole_block_goes_out_as_soon_as_it_is_read() {
        let given = words("ibs=4 obs=4");
        let taken = asked(&given).unwrap();
        let mut copy = Copy::new(&taken);
        let mut out = Vec::new();
        let kept = |out: &mut Vec<Vec<u8>>, bytes: &[u8]| {
            out.push(bytes.to_vec());
            Ok(())
        };
        copy.take(b"abcd".to_vec(), &mut |b| kept(&mut out, b))
            .unwrap();
        assert_eq!(out, [b"abcd".to_vec()]);
        copy.take(b"ef".to_vec(), &mut |b| kept(&mut out, b))
            .unwrap();
        assert_eq!(out, [b"abcd".to_vec()], "a part of one waits for more");
        copy.finish(&mut |b| kept(&mut out, b)).unwrap();
        assert_eq!(out, [b"abcd".to_vec(), b"ef".to_vec()]);
    }

    #[test]
    fn a_standard_input_that_cannot_be_read_ends_it() {
        let mut host = Host::new("unread");
        std::fs::create_dir(host.root.join("d")).unwrap();
        host.stdin_file = Some(std::fs::File::open(host.root.join("d")).unwrap());
        assert_eq!(
            ran(&mut host, "skip=1", b""),
            (1, Vec::new(), said("dd: -: a directory, not a file\n")),
            "it cannot go past what it cannot read"
        );
        assert_eq!(
            ran(&mut host, "", b""),
            (
                1,
                Vec::new(),
                said("dd: -: a directory, not a file\n0+0 records in\n0+0 records out\n")
            )
        );
    }
}
