//! A program started by its name, as the tools that run others start
//! one, and waited for: from the programs folder, or where there is none,
//! as on a CD, from the programs the loader brought; its input one that
//! has ended, and its output and errors the starting program's own.

use alloc::format;
use alloc::string::{String, ToString};

use crate::start::{Given, Sink};
use crate::{Ended, Error};

/// Where programs named without a path are.
pub const PROGRAMS: &str = "/System/Boot/meridian/programs";

/// How a program started by name went.
#[derive(Debug, PartialEq, Eq)]
pub enum Ran {
    Ended(Ended),
    /// Its program is not there.
    NotThere,
    /// It could not start.
    Unstarted(Error),
}

/// What SPAWN is asked for to start the program `name`: the path as it
/// is, else the file of that name among the programs, and where `has`
/// finds no such file the program of that name the loader brought.
#[must_use]
pub fn path(name: &str, has: impl FnOnce(&str) -> bool) -> String {
    if name.contains('/') {
        return name.to_string();
    }
    let path = format!("{PROGRAMS}/{name}.mexec");
    if has(&path) {
        path
    } else {
        format!("{name}.mexec")
    }
}

/// A handle for `sink` of the child's own.
fn handed(sink: Option<Sink>) -> Option<Sink> {
    match sink? {
        Sink::Channel(end) => crate::duplicate(end).ok().map(Sink::Channel),
        Sink::File(file) => crate::duplicate(file).ok().map(Sink::File),
    }
}

/// Runs the program `name` with `words`, and waits for it to end.
#[must_use]
pub fn run(name: &str, words: &[&str]) -> Ran {
    let Ok((mine, theirs)) = crate::channel() else {
        return Ran::Unstarted(Error::Full);
    };
    let _ = crate::close(mine);
    let given = Given {
        input: Some(theirs),
        output: handed(crate::start::given().output),
        error: handed(crate::start::given().error),
        line: None,
    };
    let path = path(name, |path| {
        crate::open(path, crate::abi::file::READ).is_ok_and(|file| {
            let _ = crate::close(file);
            true
        })
    });
    match crate::spawn_handing(&path, words, given) {
        Ok((child, start)) => {
            let _ = crate::close(start);
            let ended = crate::wait(child);
            let _ = crate::close(child);
            ended.map_or_else(Ran::Unstarted, Ran::Ended)
        }
        Err(Error::NotFound | Error::NoSuchProgram) => Ran::NotThere,
        Err(e) => Ran::Unstarted(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_is_found_by_its_path_or_its_name() {
        let on_disk = |path: &str| path == "/System/Boot/meridian/programs/echo.mexec";
        assert_eq!(
            path("echo", on_disk),
            "/System/Boot/meridian/programs/echo.mexec"
        );
        assert_eq!(
            path("wc", on_disk),
            "wc.mexec",
            "a program not on the disk is one the loader brought"
        );
        assert_eq!(path("/x/y.mexec", |_| false), "/x/y.mexec");
        assert_eq!(path("./y", |_| true), "./y");
        let mut asked = None;
        let _ = path("sort", |path| {
            asked = Some(path.to_string());
            false
        });
        assert_eq!(
            asked.as_deref(),
            Some("/System/Boot/meridian/programs/sort.mexec")
        );
    }
}
