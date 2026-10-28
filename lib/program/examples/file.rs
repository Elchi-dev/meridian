//! file: says what kind of file each one it is given is, from its first
//! bytes, as POSIX's file does: a directory, empty, text and which,
//! commands text for a script, programs for Meridian and for Windows and
//! UEFI, compressed data, archives, images and documents, and data for
//! the rest. `-d` and `-h` change nothing, as there are no links, and
//! `-i` says regular file for every file that is not empty. It ends with
//! 0, and with 1 when a file could not be read.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use program::{Error, Handle, text};

program::main!(file);

/// How many bytes of a file are looked at.
const LOOKED: usize = 4096;

/// What file's words ask for.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'a> {
    /// `-i`: no kind past regular file.
    plain: bool,
    files: &'a [String],
}

/// What file's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut plain = false;
    let mut words = arguments;
    while let Some((word, rest)) = words.split_first() {
        if word == "--" {
            words = rest;
            break;
        }
        if !word.starts_with('-') || word == "-" {
            break;
        }
        for letter in word[1..].chars() {
            match letter {
                'd' | 'h' => {}
                'i' => plain = true,
                'm' | 'M' => {
                    return Err(format!(
                        "file: -{letter}: files of magic numbers are not read here\n"
                    ));
                }
                _ => return Err(format!("file: -{letter} is no option\n")),
            }
        }
        words = rest;
    }
    if words.is_empty() {
        return Err(String::from("file: names a file at least\n"));
    }
    Ok(Asked {
        plain,
        files: words,
    })
}

fn u16_at(bytes: &[u8], at: usize, big: bool) -> Option<u16> {
    let two: [u8; 2] = bytes.get(at..at.checked_add(2)?)?.try_into().ok()?;
    Some(if big {
        u16::from_be_bytes(two)
    } else {
        u16::from_le_bytes(two)
    })
}

fn u32_at(bytes: &[u8], at: usize, big: bool) -> Option<u32> {
    let four: [u8; 4] = bytes.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(if big {
        u32::from_be_bytes(four)
    } else {
        u32::from_le_bytes(four)
    })
}

/// The machine a program's header names.
fn machine(code: u32) -> &'static str {
    match code {
        3 | 0x14c => "x86",
        62 | 0x8664 => "x86-64",
        40 | 0x1c0 => "ARM",
        183 | 0xaa64 => "AArch64",
        243 | 0x5064 => "RISC-V",
        _ => "a machine not known here",
    }
}

/// An ELF file: what it is, its bits and its machine.
fn elf(bytes: &[u8]) -> Option<String> {
    if !bytes.starts_with(b"\x7fELF") {
        return None;
    }
    let bits = match bytes.get(4) {
        Some(1) => "32-bit",
        Some(2) => "64-bit",
        _ => return Some(String::from("ELF file of a class not known here")),
    };
    let big = bytes.get(5) == Some(&2);
    let what = match u16_at(bytes, 16, big)? {
        1 => "relocatable",
        2 => "program",
        3 => "position-independent program or shared object",
        4 => "core dump",
        _ => "file",
    };
    let on = machine(u32::from(u16_at(bytes, 18, big)?));
    Some(format!("ELF {bits} {what} for {on}"))
}

/// A program in Microsoft's format: for DOS, or past its DOS header for
/// Windows or UEFI.
fn pe(bytes: &[u8]) -> Option<String> {
    if !bytes.starts_with(b"MZ") {
        return None;
    }
    let dos = String::from("program for MS-DOS");
    let Some(at) = u32_at(bytes, 0x3c, false).and_then(|at| usize::try_from(at).ok()) else {
        return Some(dos);
    };
    if bytes.get(at..at + 4) != Some(b"PE\0\0") {
        return Some(dos);
    }
    let on = machine(u32::from(u16_at(bytes, at + 4, false)?));
    let library = u16_at(bytes, at + 22, false)? & 0x2000 != 0;
    let (form, subsystem_at) = match u16_at(bytes, at + 24, false)? {
        0x10b => ("PE32", at + 24 + 68),
        0x20b => ("PE32+", at + 24 + 68),
        _ => return Some(format!("PE file for {on}")),
    };
    let kind = match (u16_at(bytes, subsystem_at, false)?, library) {
        (10, _) => "UEFI application",
        (11, _) => "UEFI boot driver",
        (12, _) => "UEFI runtime driver",
        (_, true) => "DLL for Windows",
        (2, false) => "window program for Windows",
        (3, false) => "console program for Windows",
        _ => "program for Windows",
    };
    Some(format!("{form} {kind}, {on}"))
}

/// Compressed data, archives, images and documents, by the bytes they
/// begin with.
fn known(bytes: &[u8]) -> Option<String> {
    let starts = |prefix: &[u8]| bytes.starts_with(prefix);
    Some(if starts(b"\x1f\x9d") {
        match bytes.get(2) {
            Some(b) => format!(
                "data compressed by compress, codes of up to {} bits",
                b & 0x1f
            ),
            None => String::from("data compressed by compress"),
        }
    } else if starts(b"\x1f\x8b") {
        String::from("data compressed by gzip")
    } else if starts(b"BZh") {
        String::from("data compressed by bzip2")
    } else if starts(b"\xfd7zXZ\0") {
        String::from("data compressed by xz")
    } else if starts(b"\x28\xb5\x2f\xfd") {
        String::from("data compressed by zstd")
    } else if starts(b"PK\x03\x04") || starts(b"PK\x05\x06") {
        String::from("Zip archive")
    } else if bytes.get(257..262) == Some(b"ustar") {
        String::from("tar archive")
    } else if starts(b"070707") || starts(b"070701") || starts(b"070702") {
        String::from("cpio archive")
    } else if starts(b"!<arch>\n") {
        String::from("ar archive")
    } else if starts(b"\x89PNG\r\n\x1a\n") {
        let (w, h) = (u32_at(bytes, 16, true)?, u32_at(bytes, 20, true)?);
        format!("PNG image, {w} x {h}")
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        let (w, h) = (u16_at(bytes, 6, false)?, u16_at(bytes, 8, false)?);
        format!("GIF image, {w} x {h}")
    } else if starts(b"\xff\xd8\xff") {
        String::from("JPEG image")
    } else if starts(b"%PDF-") {
        let version: String = bytes[5..]
            .iter()
            .take_while(|b| b.is_ascii_digit() || **b == b'.')
            .map(|&b| char::from(b))
            .collect();
        format!("PDF document, version {version}")
    } else {
        return None;
    })
}

/// Text: all of it characters that print or space it out; ASCII or
/// UTF-8, and a script with the command after its `#!`. When `cut`, the
/// end of what was looked at may cut its last character in two.
fn text(bytes: &[u8], cut: bool) -> Option<String> {
    let text = match core::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) if cut && e.error_len().is_none() => {
            core::str::from_utf8(&bytes[..e.valid_up_to()]).ok()?
        }
        Err(_) => return None,
    };
    let spacing = |c: char| matches!(c, '\n' | '\t' | '\r' | '\x0c' | '\x0b' | '\x1b' | '\x08');
    if text.chars().any(|c| c.is_control() && !spacing(c)) {
        return None;
    }
    if let Some(rest) = text.strip_prefix("#!") {
        let command = rest.split_whitespace().next().unwrap_or("");
        return Some(format!("commands text for {command}"));
    }
    Some(String::from(if text.is_ascii() {
        "ASCII text"
    } else {
        "UTF-8 text"
    }))
}

/// What kind of file the first bytes `bytes` of one make it, all of it
/// when `whole`.
fn kind(bytes: &[u8], whole: bool) -> String {
    if bytes.is_empty() {
        return String::from("empty");
    }
    elf(bytes)
        .or_else(|| pe(bytes))
        .or_else(|| known(bytes))
        .or_else(|| text(bytes, !whole))
        .unwrap_or_else(|| String::from("data"))
}

/// What file sees of a path.
#[derive(Debug, PartialEq, Eq)]
enum Look {
    Directory,
    /// The first bytes of a file, and whether they are all of it.
    File(Vec<u8>, bool),
}

/// What file reads files with.
trait Files {
    /// What is at `path`.
    ///
    /// # Errors
    ///
    /// When nothing can be read there.
    fn look(&mut self, path: &str) -> Result<Look, Error>;
}

/// The line file writes for `path`, or what it says when it cannot read
/// it.
fn line(files: &mut impl Files, path: &str, plain: bool) -> Result<String, String> {
    let kind = match files
        .look(path)
        .map_err(|e| text::trouble("file", path, e))?
    {
        Look::Directory => String::from("directory"),
        Look::File(bytes, _) if bytes.is_empty() => String::from("empty"),
        Look::File(_, _) if plain => String::from("regular file"),
        Look::File(bytes, whole) => kind(&bytes, whole),
    };
    Ok(format!("{path}: {kind}\n"))
}

/// Meridian's files.
struct System;

impl Files for System {
    fn look(&mut self, path: &str) -> Result<Look, Error> {
        let handle = program::open(path, program::abi::file::READ)?;
        let info = program::info(handle);
        let looked = info.and_then(|info| {
            if info.directory {
                return Ok(Look::Directory);
            }
            let mut bytes = Vec::new();
            let mut room = [0u8; program::abi::MESSAGE_BYTES];
            while bytes.len() < LOOKED {
                let len = program::read(handle, bytes.len() as u64, &mut room)?;
                if len == 0 {
                    break;
                }
                bytes.extend_from_slice(&room[..len]);
            }
            bytes.truncate(LOOKED);
            let whole = info.size <= LOOKED as u64;
            Ok(Look::File(bytes, whole))
        });
        let _ = program::close(handle);
        looked
    }
}

#[cfg_attr(test, allow(dead_code))]
fn file(_: Handle) -> u64 {
    let asked = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
    let mut out = text::standard_output();
    let mut failed = false;
    for path in asked.files {
        match line(&mut System, path, asked.plain) {
            Ok(line) => out.put(line.as_bytes()),
            Err(why) => {
                out.flush();
                let _ = program::write_error(&why);
                failed = true;
            }
        }
    }
    u64::from(!out.finish() || failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split(' ')
            .filter(|w| !w.is_empty())
            .map(String::from)
            .collect()
    }

    /// The first bytes of an ELF file of `class`, its numbers big-endian
    /// when `big`, of the kind and machine given.
    fn elf_header(class: u8, big: bool, kind: u16, machine: u16) -> Vec<u8> {
        let mut h = alloc::vec![0u8; 64];
        h[..4].copy_from_slice(b"\x7fELF");
        h[4] = class;
        h[5] = if big { 2 } else { 1 };
        h[6] = 1;
        let order = |v: u16| {
            if big {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        h[16..18].copy_from_slice(&order(kind));
        h[18..20].copy_from_slice(&order(machine));
        h
    }

    /// The first bytes of a PE file, its PE header at 0x40, as Microsoft's
    /// specification lays them out.
    fn pe_header(machine: u16, characteristics: u16, magic: u16, subsystem: u16) -> Vec<u8> {
        let mut h = alloc::vec![0u8; 0x40 + 24 + 96];
        h[..2].copy_from_slice(b"MZ");
        h[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        h[0x40..0x44].copy_from_slice(b"PE\0\0");
        h[0x44..0x46].copy_from_slice(&machine.to_le_bytes());
        h[0x40 + 22..0x40 + 24].copy_from_slice(&characteristics.to_le_bytes());
        h[0x40 + 24..0x40 + 26].copy_from_slice(&magic.to_le_bytes());
        h[0x40 + 24 + 68..0x40 + 24 + 70].copy_from_slice(&subsystem.to_le_bytes());
        h
    }

    #[test]
    fn its_words_are_options_then_files() {
        let given = words("-dh -i a b");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                plain: true,
                files: &given[2..]
            })
        );
        let given = words("-- -i");
        assert_eq!(
            asked(&given),
            Ok(Asked {
                plain: false,
                files: &given[1..]
            })
        );
        let given = words("-");
        assert_eq!(asked(&given).map(|a| a.files), Ok(&given[..]));
        assert_eq!(
            asked(&words("")),
            Err("file: names a file at least\n".into())
        );
        assert_eq!(
            asked(&words("-d")),
            Err("file: names a file at least\n".into())
        );
        assert_eq!(asked(&words("-x a")), Err("file: -x is no option\n".into()));
        assert_eq!(
            asked(&words("-m magic a")),
            Err("file: -m: files of magic numbers are not read here\n".into())
        );
        assert_eq!(
            asked(&words("-M magic a")),
            Err("file: -M: files of magic numbers are not read here\n".into())
        );
    }

    #[test]
    fn elf_files_say_their_bits_kind_and_machine() {
        for (class, big, kind, machine, said) in [
            (2, false, 2, 62, "ELF 64-bit program for x86-64"),
            (1, false, 1, 40, "ELF 32-bit relocatable for ARM"),
            (
                2,
                true,
                3,
                183,
                "ELF 64-bit position-independent program or shared object for AArch64",
            ),
            (2, false, 4, 243, "ELF 64-bit core dump for RISC-V"),
            (1, false, 2, 3, "ELF 32-bit program for x86"),
            (
                2,
                false,
                9,
                999,
                "ELF 64-bit file for a machine not known here",
            ),
        ] {
            assert_eq!(kind_of(&elf_header(class, big, kind, machine)), said);
        }
        assert_eq!(
            kind_of(&elf_header(3, false, 2, 62)),
            "ELF file of a class not known here"
        );
        assert_eq!(
            kind_of(b"\x7fELF\x02\x01"),
            "data",
            "a header cut short is none"
        );
    }

    fn kind_of(bytes: &[u8]) -> String {
        kind(bytes, true)
    }

    #[test]
    fn microsoft_s_programs_say_their_form_and_kind() {
        for (machine, characteristics, magic, subsystem, said) in [
            (
                0x8664,
                0,
                0x20b,
                3,
                "PE32+ console program for Windows, x86-64",
            ),
            (
                0x8664,
                0,
                0x20b,
                2,
                "PE32+ window program for Windows, x86-64",
            ),
            (0x14c, 0, 0x10b, 3, "PE32 console program for Windows, x86"),
            (0x8664, 0x2000, 0x20b, 3, "PE32+ DLL for Windows, x86-64"),
            (0x8664, 0, 0x20b, 10, "PE32+ UEFI application, x86-64"),
            (0xaa64, 0, 0x20b, 11, "PE32+ UEFI boot driver, AArch64"),
            (0x8664, 0, 0x20b, 12, "PE32+ UEFI runtime driver, x86-64"),
            (0x8664, 0, 0x20b, 9, "PE32+ program for Windows, x86-64"),
            (0x8664, 0, 0x107, 3, "PE file for x86-64"),
        ] {
            assert_eq!(
                kind_of(&pe_header(machine, characteristics, magic, subsystem)),
                said
            );
        }
        let mut dos = alloc::vec![0u8; 0x40];
        dos[..2].copy_from_slice(b"MZ");
        assert_eq!(kind_of(&dos), "program for MS-DOS", "no PE where it points");
        dos[0x3c..0x40].copy_from_slice(&0x1000u32.to_le_bytes());
        assert_eq!(
            kind_of(&dos),
            "program for MS-DOS",
            "pointing past what was read"
        );
        assert_eq!(kind_of(b"MZ"), "program for MS-DOS");
    }

    #[test]
    fn data_archives_images_and_documents_are_known_by_their_start() {
        let mut tar = alloc::vec![0u8; 512];
        tar[257..263].copy_from_slice(b"ustar\0");
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&480u32.to_be_bytes());
        for (bytes, said) in [
            (
                &b"\x1f\x9d\x90\x61"[..],
                "data compressed by compress, codes of up to 16 bits",
            ),
            (
                b"\x1f\x9d\x8c",
                "data compressed by compress, codes of up to 12 bits",
            ),
            (b"\x1f\x8b\x08\0", "data compressed by gzip"),
            (b"BZh91AY", "data compressed by bzip2"),
            (b"\xfd7zXZ\0\0", "data compressed by xz"),
            (b"\x28\xb5\x2f\xfd\x00", "data compressed by zstd"),
            (b"PK\x03\x04\x14\0", "Zip archive"),
            (b"PK\x05\x06\0\0", "Zip archive"),
            (&tar, "tar archive"),
            (b"070707000001", "cpio archive"),
            (b"070701000001", "cpio archive"),
            (b"!<arch>\nfoo", "ar archive"),
            (&png, "PNG image, 640 x 480"),
            (b"GIF89a\x20\x03\x58\x02", "GIF image, 800 x 600"),
            (b"\xff\xd8\xff\xe0\0\x10JFIF", "JPEG image"),
            (b"%PDF-1.7\n%", "PDF document, version 1.7"),
        ] {
            assert_eq!(kind_of(bytes), said);
        }
        assert_eq!(kind_of(b"\x89PNG\r\n\x1a\n\0"), "data", "a PNG cut short");
        assert_eq!(kind_of(b"GIF89a\0"), "data");
        assert_eq!(
            kind_of(b"GIF89a "),
            "ASCII text",
            "cut short in characters that print, it is text"
        );
    }

    #[test]
    fn text_is_ascii_utf_8_or_a_script() {
        assert_eq!(kind_of(b"hello\nworld\n"), "ASCII text");
        assert_eq!(kind_of(b"tab\there\r\n\x0c"), "ASCII text");
        assert_eq!(
            kind_of(b"\x1b[1mbold\x1b[0m\n"),
            "ASCII text",
            "escapes are text"
        );
        assert_eq!(kind_of("gr\u{fc}\u{df}e\n".as_bytes()), "UTF-8 text");
        assert_eq!(
            kind_of(b"#!/bin/sh\necho hi\n"),
            "commands text for /bin/sh"
        );
        assert_eq!(
            kind_of(b"#! /usr/bin/awk -f\n"),
            "commands text for /usr/bin/awk"
        );
        assert_eq!(kind_of(b"#!"), "commands text for ");
        assert_eq!(kind_of(b"a\0b"), "data", "a zero is no text");
        assert_eq!(kind_of(b"latin \xe9\n"), "data", "nor a byte UTF-8 has not");
        assert_eq!(kind_of(b"bell\x07"), "data");
        let cut = "ab\u{20ac}".as_bytes();
        assert_eq!(
            kind(&cut[..3], false),
            "ASCII text",
            "a character cut by the end"
        );
        assert_eq!(kind(&cut[..4], false), "ASCII text");
        assert_eq!(kind(&cut[..4], true), "data", "but not in a whole file");
        assert_eq!(kind(b"", true), "empty");
    }

    /// Files among the host's temporary ones, gone once dropped.
    struct Host {
        root: std::path::PathBuf,
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    impl Files for Host {
        fn look(&mut self, path: &str) -> Result<Look, Error> {
            assert!(
                !path.contains(".."),
                "{path} climbs out of the test's directory"
            );
            let at = self.root.join(path.trim_start_matches('/'));
            let found = std::fs::metadata(&at).map_err(|_| Error::NotFound)?;
            if found.is_dir() {
                return Ok(Look::Directory);
            }
            let bytes = std::fs::read(&at).map_err(|_| Error::Unreadable)?;
            let whole = bytes.len() <= LOOKED;
            Ok(Look::File(bytes.into_iter().take(LOOKED).collect(), whole))
        }
    }

    #[test]
    fn each_file_gets_its_line() {
        let root = std::env::temp_dir().join(format!("meridian-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("d")).unwrap();
        std::fs::write(root.join("e"), b"").unwrap();
        std::fs::write(root.join("t"), b"text\n").unwrap();
        let mut long = alloc::vec![b'a'; LOOKED - 1];
        long.extend_from_slice("\u{20ac}".as_bytes());
        std::fs::write(root.join("long"), &long).unwrap();
        let mut host = Host { root };
        assert_eq!(line(&mut host, "d", false), Ok("d: directory\n".into()));
        assert_eq!(line(&mut host, "e", false), Ok("e: empty\n".into()));
        assert_eq!(line(&mut host, "t", false), Ok("t: ASCII text\n".into()));
        assert_eq!(line(&mut host, "t", true), Ok("t: regular file\n".into()));
        assert_eq!(
            line(&mut host, "e", true),
            Ok("e: empty\n".into()),
            "empty still"
        );
        assert_eq!(line(&mut host, "d", true), Ok("d: directory\n".into()));
        assert_eq!(
            line(&mut host, "long", false),
            Ok("long: ASCII text\n".into()),
            "its euro cut by the bytes looked at"
        );
        assert_eq!(
            line(&mut host, "nope", false),
            Err("file: nope: no such file\n".into())
        );
    }

    #[test]
    fn real_files_on_the_host_are_what_they_are() {
        let ls = std::fs::read("/usr/bin/ls").unwrap();
        let said = kind_of(&ls[..LOOKED.min(ls.len())]);
        assert!(
            said.starts_with("ELF 64-bit ") && said.ends_with(" for x86-64"),
            "{said}"
        );
        let dir = std::env::temp_dir().join(format!("meridian-file-gz-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("t"), b"some text\n").unwrap();
        let status = std::process::Command::new("gzip")
            .arg(dir.join("t"))
            .status()
            .unwrap();
        assert!(status.success());
        let gz = std::fs::read(dir.join("t.gz")).unwrap();
        assert_eq!(kind_of(&gz), "data compressed by gzip");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn headers_cut_short_and_every_machine_are_said_as_they_are() {
        let whole = pe_header(0x8664, 0, 0x20b, 3);
        for cut in [0x40 + 4, 0x40 + 6, 0x40 + 23, 0x40 + 25, 0x40 + 24 + 69] {
            assert_eq!(kind_of(&whole[..cut]), "data", "PE cut at {cut}");
        }
        for (code, on) in [
            (0x14c, "x86"),
            (0x8664, "x86-64"),
            (0x1c0, "ARM"),
            (0xaa64, "AArch64"),
            (0x5064, "RISC-V"),
            (0x1234, "a machine not known here"),
        ] {
            assert_eq!(
                kind_of(&pe_header(code, 0, 0x20b, 3)),
                format!("PE32+ console program for Windows, {on}")
            );
        }
        for (code, on) in [
            (3, "x86"),
            (62, "x86-64"),
            (40, "ARM"),
            (183, "AArch64"),
            (243, "RISC-V"),
        ] {
            assert_eq!(
                kind_of(&elf_header(2, true, 2, code)),
                format!("ELF 64-bit program for {on}")
            );
        }
        assert_eq!(
            kind_of(&elf_header(1, true, 1, 40)),
            "ELF 32-bit relocatable for ARM",
            "big-endian"
        );
        assert_eq!(
            kind_of(&elf_header(2, false, 0, 62)),
            "ELF 64-bit file for x86-64"
        );
        assert_eq!(kind_of(b"\x1f\x9d"), "data compressed by compress");
        assert_eq!(kind_of(b"%PDF-x"), "PDF document, version ");
        assert_eq!(kind_of(b"%PDF-2.0"), "PDF document, version 2.0");
    }

    #[test]
    fn numbers_are_read_in_their_order_or_not_at_all_past_the_end() {
        let bytes = [0x12, 0x34, 0x56, 0x78];
        assert_eq!(u16_at(&bytes, 0, false), Some(0x3412));
        assert_eq!(u16_at(&bytes, 0, true), Some(0x1234));
        assert_eq!(u16_at(&bytes, 2, true), Some(0x5678));
        assert_eq!(u16_at(&bytes, 3, false), None);
        assert_eq!(u32_at(&bytes, 0, true), Some(0x1234_5678));
        assert_eq!(u32_at(&bytes, 0, false), Some(0x7856_3412));
        assert_eq!(u32_at(&bytes, 1, false), None);
        assert_eq!(
            u16_at(&bytes, usize::MAX - 1, false),
            None,
            "nor past the last place"
        );
        assert_eq!(u32_at(&bytes, usize::MAX - 2, true), None);
    }
}
