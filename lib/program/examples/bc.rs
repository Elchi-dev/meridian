//! bc: runs the bc programs in the files its operands name, one after
//! another, and then the one on its standard input, as POSIX's bc does,
//! with lib/bc. `-l` first defines the functions s, c, a, l, e and j and
//! sets scale to 20. Mistakes in a program are said and the program goes
//! on, as in GNU's bc; it ends with 0, or with 1 for words it cannot take
//! or a file it cannot read, which ends it there.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;

use bc::run::{Bc, World};
use program::Handle;
use program::text::{self, LineReader, Source};

// bc runs a program by recursion over its tree (lib/bc's limits).
program::main!(bc, stack = 256);

/// What bc's words ask for: the library, and the files to run.
fn asked(arguments: &[String]) -> Result<(bool, &[String]), String> {
    let mut library = false;
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
            if letter != 'l' {
                return Err(format!("bc: -{letter} is no option\n"));
            }
            library = true;
        }
        words = rest;
    }
    Ok((library, words))
}

/// bc's standard output and error.
struct System {
    out: text::Out<text::Write>,
}

impl World for System {
    fn write(&mut self, bytes: &[u8]) {
        self.out.put(bytes);
    }

    fn say(&mut self, text: &str) {
        // What was written before goes out before what is said.
        self.out.flush();
        let _ = program::write_error(text);
    }
}

/// Runs the lines of `source`, read as `name`, until they end or bc
/// does.
fn run_lines(
    run: &mut Bc,
    name: &str,
    source: Source,
    system: &mut System,
) -> Result<(), program::Error> {
    run.begin(name);
    let mut lines = LineReader::new(source);
    let read = loop {
        match lines.line() {
            Ok(Some(mut line)) => {
                line.push(b'\n');
                if !run.feed(&line, system) {
                    break Ok(());
                }
                // What it wrote shows before the next line is waited for.
                system.out.flush();
            }
            Ok(None) => break Ok(()),
            Err(e) => break Err(e),
        }
    };
    run.end(system);
    lines.into_pieces().close();
    read
}

#[cfg_attr(test, allow(dead_code))]
fn bc(_: Handle) -> u64 {
    let (library, files) = match asked(program::arguments()) {
        Ok(asked) => asked,
        Err(why) => {
            let _ = program::write_error(&why);
            return 1;
        }
    };
    let mut system = System {
        out: text::standard_output(),
    };
    let mut run = Bc::new();
    if library {
        run.begin("the library");
        run.feed(bc::LIBRARY.as_bytes(), &mut system);
        run.end(&mut system);
        run.set_scale(20);
    }
    let mut status = 0;
    // The files, and the standard input after them, which - names.
    for name in files.iter().map(String::as_str).chain(["-"]) {
        if run.ended() {
            break;
        }
        let ran =
            Source::open(name).and_then(|source| run_lines(&mut run, name, source, &mut system));
        if let Err(e) = ran {
            system.say(&text::trouble("bc", name, e));
            status = 1;
            break;
        }
    }
    if !system.out.finish() {
        status = 1;
    }
    status
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

    #[test]
    fn its_words_are_l_and_files() {
        let given = words("-l a b");
        assert_eq!(asked(&given), Ok((true, &given[1..])));
        let given = words("a -l");
        assert_eq!(
            asked(&given),
            Ok((false, &given[..])),
            "no option after a file"
        );
        let given = words("-ll");
        assert_eq!(asked(&given), Ok((true, &given[1..])));
        let given = words("-- -l");
        assert_eq!(asked(&given), Ok((false, &given[1..])));
        let given = words("-");
        assert_eq!(asked(&given), Ok((false, &given[..])), "- is a file");
        let given = words("");
        assert_eq!(asked(&given), Ok((false, &given[..])));
        assert_eq!(asked(&words("-q")), Err("bc: -q is no option\n".into()));
        assert_eq!(asked(&words("-lx f")), Err("bc: -x is no option\n".into()));
    }
}
