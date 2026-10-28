//! ls: writes the names of the files its operands name and of what the
//! directories among them hold, as POSIX's ls does: the files first,
//! then each directory under its name when there are more, in the order
//! of their names' bytes, by size with `-S`, as they are with `-f`, the
//! other way round with `-r`. Names that start with `.` show with `-a`,
//! which adds `.` and `..`, and with `-A`. `-1` writes one name a line,
//! as ls does when it writes into a pipe, `-C` in columns down and `-x`
//! across 80 characters, and `-m` parted by commas; `-F` and `-p` put a
//! `/` after a directory, `-s` the blocks of 512 bytes, or 1024 with
//! `-k`, a file takes before it, `-q` a `?` for a character that does not
//! print, `-R` the directories within after, and `-d` a directory as
//! itself. Meridian's files have no owners, modes, times, links or
//! numbers yet, so the options that write those are refused; with no
//! symbolic links `-H` and `-L` change nothing. It ends with 0, or 1 when
//! something could not be read.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write as _;

use program::{Error, Handle, text};

program::main!(ls);

/// Which names that start with `.` show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hidden {
    None,
    /// All but `.` and `..`, as `-A` asks.
    AlmostAll,
    /// All, `.` and `..` too, as `-a` asks.
    All,
}

/// How the names are put on lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Form {
    Lines,
    Columns,
    Across,
    Commas,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Order {
    Name,
    Size,
    /// As the directory lists them, as `-f` asks.
    Listed,
}

/// What ls's words ask for.
#[derive(Debug, PartialEq, Eq)]
struct Asked<'w> {
    hidden: Hidden,
    form: Form,
    order: Order,
    reverse: bool,
    marks: Marks,
    /// The bytes of a block `-s` counts in, when it is given.
    blocks: Option<u64>,
    walk: Walk,
    paths: &'w [String],
}

/// What is written with a name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Marks {
    /// Whether a directory gets a `/` after its name.
    slash: bool,
    /// Whether a character that does not print is written `?`.
    printable: bool,
}

/// Where ls goes from the paths it is given.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Walk {
    /// Into the directories within, as `-R` asks.
    deep: bool,
    /// Nowhere: a directory is listed as itself, as `-d` asks.
    itself: bool,
}

/// What ls's words ask for, or why they cannot be taken.
fn asked(arguments: &[String]) -> Result<Asked<'_>, String> {
    let mut asked = Asked {
        hidden: Hidden::None,
        form: Form::Lines,
        order: Order::Name,
        reverse: false,
        marks: Marks::default(),
        blocks: None,
        walk: Walk::default(),
        paths: &[],
    };
    let mut kilobytes = false;
    let mut sized = false;
    let mut words = arguments;
    while let Some((word, rest)) = words.split_first() {
        if word == "--" {
            words = rest;
            break;
        }
        let Some(letters) = word.strip_prefix('-').filter(|l| !l.is_empty()) else {
            break;
        };
        for letter in letters.chars() {
            match letter {
                'a' => asked.hidden = Hidden::All,
                'A' => asked.hidden = Hidden::AlmostAll,
                '1' => asked.form = Form::Lines,
                'C' => asked.form = Form::Columns,
                'x' => asked.form = Form::Across,
                'm' => asked.form = Form::Commas,
                'F' | 'p' => asked.marks.slash = true,
                'S' => asked.order = Order::Size,
                // -f lists as it finds, every name, nothing else asked.
                'f' => {
                    asked.order = Order::Listed;
                    asked.hidden = Hidden::All;
                }
                'r' => asked.reverse = true,
                's' => sized = true,
                'k' => kilobytes = true,
                'q' => asked.marks.printable = true,
                'R' => asked.walk.deep = true,
                'd' => asked.walk.itself = true,
                'H' | 'L' => {}
                'i' | 'l' | 'n' | 'g' | 'o' | 't' | 'c' | 'u' => {
                    return Err(format!(
                        "ls: -{letter}: Meridian's files have no owners, modes, times, links or numbers yet\n"
                    ));
                }
                _ => return Err(format!("ls: -{letter} is no option\n")),
            }
        }
        words = rest;
    }
    if asked.order == Order::Listed {
        (asked.reverse, sized) = (false, false);
    }
    asked.blocks = sized.then_some(if kilobytes { 1024 } else { 512 });
    asked.paths = words;
    Ok(asked)
}

/// What a file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Found {
    directory: bool,
    size: u64,
}

/// What ls does with files.
trait Files {
    /// What is at `path`.
    fn found(&mut self, path: &str) -> Result<Found, Error>;

    /// The names in the directory at `path`, as it lists them.
    fn names(&mut self, path: &str) -> Result<Vec<String>, Error>;

    /// Writes `text` on the standard output.
    fn write(&mut self, text: &str);

    /// Says `text` on the standard error.
    fn say(&mut self, text: &str);
}

/// `name` in the directory at `directory`.
fn joined(directory: &str, name: &str) -> String {
    if directory.ends_with('/') {
        format!("{directory}{name}")
    } else {
        format!("{directory}/{name}")
    }
}

/// A name to write, and what it names.
struct Entry {
    name: String,
    found: Found,
}

/// Whether the name shows as `hidden` says.
fn shows(name: &str, hidden: Hidden) -> bool {
    match hidden {
        Hidden::All => true,
        Hidden::AlmostAll => name != "." && name != "..",
        Hidden::None => !name.starts_with('.'),
    }
}

/// `entries` in the order `asked` says.
fn ordered(mut entries: Vec<Entry>, asked: &Asked) -> Vec<Entry> {
    match asked.order {
        Order::Listed => return entries,
        Order::Name => entries.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes())),
        Order::Size => entries.sort_by(|a, b| {
            b.found
                .size
                .cmp(&a.found.size)
                .then_with(|| a.name.as_bytes().cmp(b.name.as_bytes()))
        }),
    }
    if asked.reverse {
        entries.reverse();
    }
    entries
}

/// How `entry` is written, without what parts it from the next.
fn shown(entry: &Entry, asked: &Asked) -> String {
    let mut name = if asked.marks.printable {
        entry
            .name
            .chars()
            .map(|c| if c.is_control() { '?' } else { c })
            .collect()
    } else {
        entry.name.clone()
    };
    if asked.marks.slash && entry.found.directory {
        name.push('/');
    }
    match asked.blocks {
        Some(block) => format!("{} {name}", entry.found.size.div_ceil(block)),
        None => name,
    }
}

/// How wide the lines `-C`, `-x` and `-m` fill are.
const WIDTH: usize = 80;

/// `names` put on lines as `form` says.
fn laid_out(names: &[String], form: Form) -> String {
    let mut text = String::new();
    if names.is_empty() {
        return text;
    }
    let widths: Vec<usize> = names.iter().map(|n| n.chars().count()).collect();
    match form {
        Form::Lines => {
            for name in names {
                text.push_str(name);
                text.push('\n');
            }
        }
        Form::Commas => {
            let mut column = 0;
            for (at, name) in names.iter().enumerate() {
                let last = at + 1 == names.len();
                let piece = if last {
                    name.clone()
                } else {
                    format!("{name},")
                };
                let len = piece.chars().count();
                if column > 0 && column + 1 + len > WIDTH {
                    text.push('\n');
                    column = 0;
                } else if column > 0 {
                    text.push(' ');
                    column += 1;
                }
                text.push_str(&piece);
                column += len;
            }
            text.push('\n');
        }
        Form::Columns | Form::Across => {
            // Each column is as wide as the widest name and two blanks.
            let cell = widths.iter().copied().max().unwrap_or(0) + 2;
            // As many columns as fit; down them the last ones may be empty.
            let columns = (WIDTH / cell).max(1);
            let rows = names.len().div_ceil(columns);
            for row in 0..rows {
                let mut line = String::new();
                for column in 0..columns {
                    let at = if form == Form::Columns {
                        column * rows + row
                    } else {
                        row * columns + column
                    };
                    let Some(name) = names.get(at) else {
                        continue;
                    };
                    line.push_str(name);
                    line.extend(core::iter::repeat_n(' ', cell - widths[at]));
                }
                text.push_str(line.trim_end());
                text.push('\n');
            }
        }
    }
    text
}

/// What the directory at `path` holds, as `asked` shows it; none when it
/// could not be read, which is said.
fn held(files: &mut impl Files, asked: &Asked, path: &str) -> Option<Vec<Entry>> {
    let names = match files.names(path) {
        Ok(names) => names,
        Err(e) => {
            files.say(&text::trouble("ls", path, e));
            return None;
        }
    };
    // Every directory holds itself and the one above; which names show
    // is for `shows` to say.
    let mut all = Vec::from([String::from("."), String::from("..")]);
    all.extend(names);
    let mut entries = Vec::new();
    for name in all.into_iter().filter(|name| shows(name, asked.hidden)) {
        let found = match name.as_str() {
            "." | ".." => Found {
                directory: true,
                size: 0,
            },
            _ => files.found(&joined(path, &name)).unwrap_or(Found {
                directory: false,
                size: 0,
            }),
        };
        entries.push(Entry { name, found });
    }
    Some(ordered(entries, asked))
}

/// Writes what the directory at `path` holds, under its name when
/// `titled`, and with `-R` the directories within after; whether all of
/// it could be read.
fn list_directory(
    files: &mut impl Files,
    asked: &Asked,
    path: &str,
    titled: bool,
    first: &mut bool,
) -> bool {
    let Some(entries) = held(files, asked, path) else {
        return false;
    };
    let mut text = String::new();
    if !*first {
        text.push('\n');
    }
    *first = false;
    if titled {
        let _ = writeln!(text, "{path}:");
    }
    if let Some(block) = asked.blocks {
        let total: u64 = entries.iter().map(|e| e.found.size.div_ceil(block)).sum();
        let _ = writeln!(text, "total {total}");
    }
    let names: Vec<String> = entries.iter().map(|e| shown(e, asked)).collect();
    text.push_str(&laid_out(&names, asked.form));
    files.write(&text);
    let mut all = true;
    if asked.walk.deep {
        for entry in &entries {
            if entry.found.directory && entry.name != "." && entry.name != ".." {
                all &= list_directory(files, asked, &joined(path, &entry.name), true, first);
            }
        }
    }
    all
}

/// Lists what `asked` names: its files, then its directories; whether
/// everything could be read.
fn list(files: &mut impl Files, asked: &Asked) -> bool {
    let dot = [String::from(".")];
    let paths = if asked.paths.is_empty() {
        &dot[..]
    } else {
        asked.paths
    };
    let mut all = true;
    let mut lone = Vec::new();
    let mut directories = Vec::new();
    for path in paths {
        match files.found(path) {
            Ok(found) if found.directory && !asked.walk.itself => directories.push(Entry {
                name: path.clone(),
                found,
            }),
            Ok(found) => lone.push(Entry {
                name: path.clone(),
                found,
            }),
            Err(e) => {
                files.say(&text::trouble("ls", path, e));
                all = false;
            }
        }
    }
    let mut first = true;
    if !lone.is_empty() {
        let lone = ordered(lone, asked);
        let names: Vec<String> = lone.iter().map(|e| shown(e, asked)).collect();
        files.write(&laid_out(&names, asked.form));
        first = false;
    }
    let titled = paths.len() > 1 || asked.walk.deep;
    for directory in ordered(directories, asked) {
        all &= list_directory(files, asked, &directory.name, titled, &mut first);
    }
    all
}

/// Meridian's files, and ls's own output.
struct System {
    out: text::Out<text::Write>,
}

impl Files for System {
    fn found(&mut self, path: &str) -> Result<Found, Error> {
        let file = program::open(path, program::abi::file::READ)?;
        let info = program::info(file);
        let _ = program::close(file);
        let info = info?;
        Ok(Found {
            directory: info.directory,
            size: info.size,
        })
    }

    fn names(&mut self, path: &str) -> Result<Vec<String>, Error> {
        let directory = program::open(path, program::abi::file::READ)?;
        let mut names = Vec::new();
        let mut room = [0u8; 1024];
        let mut index = 0;
        let listed = loop {
            match program::list(directory, index, &mut room) {
                Ok((len, _)) => names.push(String::from_utf8_lossy(&room[..len]).into_owned()),
                Err(Error::NotFound) => break Ok(names),
                Err(e) => break Err(e),
            }
            index += 1;
        };
        let _ = program::close(directory);
        listed
    }

    fn write(&mut self, text: &str) {
        self.out.put(text.as_bytes());
    }

    fn say(&mut self, text: &str) {
        self.out.flush();
        let _ = program::write_error(text);
    }
}

#[cfg_attr(test, allow(dead_code))]
fn ls(_: Handle) -> u64 {
    match asked(program::arguments()) {
        Ok(asked) => {
            let mut system = System {
                out: text::standard_output(),
            };
            let all = list(&mut system, &asked);
            let written = system.out.finish();
            u64::from(!(all && written))
        }
        Err(why) => {
            let _ = program::write_error(&why);
            1
        }
    }
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

    /// Where `path` is under `root`. One that climbs out of it stops the
    /// test, so that no mistake in ls reaches the host's own files.
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

    /// Files among the host's temporary ones, gone once dropped, and what
    /// ls wrote and said.
    struct Host {
        root: std::path::PathBuf,
        out: String,
        said: String,
    }

    impl Host {
        /// The files `paths` name, those ending in `/` directories, each
        /// file holding as many bytes as its name has.
        fn new(name: &str, paths: &[&str]) -> Host {
            let root =
                std::env::temp_dir().join(format!("meridian-ls-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            for path in paths {
                let at = root.join(path.trim_end_matches('/'));
                if path.ends_with('/') {
                    std::fs::create_dir_all(at).unwrap();
                } else {
                    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
                    std::fs::write(at, path.as_bytes()).unwrap();
                }
            }
            Host {
                root,
                out: String::new(),
                said: String::new(),
            }
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
            std::io::ErrorKind::NotADirectory => Error::NotADirectory,
            _ => Error::Unreadable,
        }
    }

    impl Files for Host {
        fn found(&mut self, path: &str) -> Result<Found, Error> {
            let meta = std::fs::metadata(confined(&self.root, path)).map_err(|e| error(&e))?;
            Ok(Found {
                directory: meta.is_dir(),
                size: if meta.is_dir() { 0 } else { meta.len() },
            })
        }

        fn names(&mut self, path: &str) -> Result<Vec<String>, Error> {
            let listed = std::fs::read_dir(confined(&self.root, path)).map_err(|e| error(&e))?;
            Ok(listed
                .map(|e| e.unwrap().file_name().into_string().unwrap())
                .collect())
        }

        fn write(&mut self, text: &str) {
            self.out.push_str(text);
        }

        fn say(&mut self, text: &str) {
            self.said.push_str(text);
        }
    }

    /// How ls with the words `text` ran on `host`: whether all could be
    /// read, what it wrote and what it said.
    fn ran(host: &mut Host, text: &str) -> (bool, String, String) {
        let given = words(text);
        let taken = asked(&given).unwrap();
        let all = list(host, &taken);
        (
            all,
            core::mem::take(&mut host.out),
            core::mem::take(&mut host.said),
        )
    }

    fn wrote(text: &str) -> (bool, String, String) {
        (true, String::from(text), String::new())
    }

    #[test]
    fn its_words_are_options_then_paths() {
        let given = words("-aRS -r x y");
        let taken = asked(&given).unwrap();
        assert_eq!(
            (
                taken.hidden,
                taken.walk.deep,
                taken.order,
                taken.reverse,
                taken.paths
            ),
            (Hidden::All, true, Order::Size, true, &given[2..])
        );
        let given = words("-A -C -F -d -q");
        let taken = asked(&given).unwrap();
        assert_eq!(
            (
                taken.hidden,
                taken.form,
                taken.marks.slash,
                taken.walk.itself,
                taken.marks.printable
            ),
            (Hidden::AlmostAll, Form::Columns, true, true, true)
        );
        assert_eq!(asked(&words("-x")).unwrap().form, Form::Across);
        assert_eq!(
            asked(&words("-m -1")).unwrap().form,
            Form::Lines,
            "the last form given counts"
        );
        assert_eq!(asked(&words("-s")).unwrap().blocks, Some(512));
        assert_eq!(asked(&words("-ks")).unwrap().blocks, Some(1024));
        assert_eq!(
            asked(&words("-k")).unwrap().blocks,
            None,
            "-k alone counts nothing"
        );
        let given = words("-rsf");
        let taken = asked(&given).unwrap();
        assert_eq!(
            (taken.order, taken.hidden, taken.reverse, taken.blocks),
            (Order::Listed, Hidden::All, false, None),
            "-f lists every name as it finds them, and nothing more"
        );
        assert_eq!(asked(&words("-p -H -L")).map(|a| a.marks.slash), Ok(true));
        let given = words("-- -a");
        assert_eq!(
            asked(&given).map(|a| (a.hidden, a.paths.len())),
            Ok((Hidden::None, 1))
        );
        assert_eq!(asked(&words("-z")), Err("ls: -z is no option\n".into()));
        for letter in ["i", "l", "n", "g", "o", "t", "c", "u"] {
            assert_eq!(
                asked(&words(&format!("-{letter}"))),
                Err(format!(
                    "ls: -{letter}: Meridian's files have no owners, modes, times, links or numbers yet\n"
                ))
            );
        }
    }

    #[test]
    fn names_are_laid_out_as_the_form_says() {
        let names: Vec<String> = ["a", "bb", "ccc", "d", "e"].map(String::from).to_vec();
        assert_eq!(laid_out(&names, Form::Lines), "a\nbb\nccc\nd\ne\n");
        assert_eq!(laid_out(&names, Form::Commas), "a, bb, ccc, d, e\n");
        assert_eq!(
            laid_out(&names, Form::Columns),
            "a    bb   ccc  d    e\n",
            "all fit on one line"
        );
        assert_eq!(laid_out(&[], Form::Columns), "");
        let long: Vec<String> = (0..10).map(|n| format!("{n:0>30}")).collect();
        assert_eq!(
            laid_out(&long, Form::Columns),
            [0, 1, 2, 3, 4]
                .map(|row| format!("{:0>30}  {:0>30}\n", row, row + 5))
                .concat(),
            "two columns of 32 fit in 80, filled down"
        );
        assert_eq!(
            laid_out(&long, Form::Across),
            [0, 2, 4, 6, 8]
                .map(|first| format!("{:0>30}  {:0>30}\n", first, first + 1))
                .concat(),
            "and across"
        );
        let wide = [format!("{:0>100}", 1), String::from("b")];
        assert_eq!(
            laid_out(&wide, Form::Columns),
            format!("{}\nb\n", wide[0]),
            "a name too wide stands alone"
        );
        let many: Vec<String> = (0..30).map(|n| format!("name{n}")).collect();
        let commas = laid_out(&many, Form::Commas);
        assert!(
            commas.lines().all(|line| line.chars().count() <= WIDTH),
            "{commas}"
        );
        assert_eq!(commas.replace('\n', " ").trim_end(), many.join(", "));
        let four: Vec<String> = (1..=4).map(|n| format!("{n:0>24}")).collect();
        assert_eq!(
            laid_out(&four, Form::Columns),
            format!("{}  {}\n{}  {}\n", four[0], four[2], four[1], four[3]),
            "three fit a line, so two rows, which two columns fill"
        );
        assert_eq!(
            laid_out(&four, Form::Across),
            format!("{}  {}  {}\n{}\n", four[0], four[1], four[2], four[3]),
            "across, the lines fill as far as they fit"
        );
        // A first name of 8 and then names of 4: the twelfth after it
        // would end at 81.
        let edge: Vec<String> = core::iter::once(String::from("abcdefgh"))
            .chain(core::iter::repeat_n(String::from("abcd"), 13))
            .collect();
        let commas = laid_out(&edge, Form::Commas);
        assert_eq!(commas.lines().next().map(|l| l.chars().count()), Some(75));
        assert!(
            commas.lines().all(|line| line.chars().count() <= WIDTH),
            "{commas}"
        );
        assert_eq!(
            laid_out(&[String::from("äöü"), String::from("x")], Form::Columns),
            "äöü  x\n",
            "widths count characters"
        );
    }

    #[test]
    fn files_come_first_then_each_directory_under_its_name() {
        let mut host = Host::new("order", &["b", "a", "d/x", "d/.h", "c/"]);
        assert_eq!(
            ran(&mut host, ""),
            wrote("a\nb\nc\nd\n"),
            "with no operand the working directory"
        );
        assert_eq!(
            ran(&mut host, "d"),
            wrote("x\n"),
            "a lone directory has no title"
        );
        assert_eq!(ran(&mut host, "d b a"), wrote("a\nb\n\nd:\nx\n"));
        assert_eq!(ran(&mut host, "c d"), wrote("c:\n\nd:\nx\n"));
        assert_eq!(
            ran(&mut host, "-d d c"),
            wrote("c\nd\n"),
            "-d lists directories as themselves"
        );
        assert_eq!(ran(&mut host, "-a d"), wrote(".\n..\n.h\nx\n"));
        assert_eq!(ran(&mut host, "-A d"), wrote(".h\nx\n"));
        assert_eq!(ran(&mut host, "-r"), wrote("d\nc\nb\na\n"));
        assert_eq!(ran(&mut host, "-p"), wrote("a\nb\nc/\nd/\n"));
        assert_eq!(ran(&mut host, "-F d/x"), wrote("d/x\n"));
        assert_eq!(ran(&mut host, "-m"), wrote("a, b, c, d\n"));
    }

    #[test]
    fn r_goes_into_the_directories_within() {
        let mut host = Host::new("deep", &["t/a", "t/u/b", "t/u/v/", "t/.w/c"]);
        assert_eq!(
            ran(&mut host, "-R t"),
            wrote("t:\na\nu\n\nt/u:\nb\nv\n\nt/u/v:\n")
        );
        assert_eq!(
            ran(&mut host, "-RA t"),
            wrote("t:\n.w\na\nu\n\nt/.w:\nc\n\nt/u:\nb\nv\n\nt/u/v:\n"),
            "with -A into hidden ones too, never into . and .."
        );
        assert_eq!(
            ran(&mut host, "-Ra t/u"),
            wrote("t/u:\n.\n..\nb\nv\n\nt/u/v:\n.\n..\n"),
            "-a's . and .. are not gone into"
        );
    }

    #[test]
    fn sizes_order_and_count_blocks() {
        let mut host = Host::new("sizes", &["a", "ccc", "bb", "e/"]);
        assert_eq!(ran(&mut host, "-S"), wrote("ccc\nbb\na\ne\n"));
        assert_eq!(ran(&mut host, "-Sr"), wrote("e\na\nbb\nccc\n"));
        assert_eq!(
            ran(&mut host, "-s"),
            wrote("total 3\n1 a\n1 bb\n1 ccc\n0 e\n")
        );
        std::fs::write(host.root.join("big"), vec![0u8; 1500]).unwrap();
        assert_eq!(
            ran(&mut host, "-s big"),
            wrote("3 big\n"),
            "in blocks of 512, the last part a block too"
        );
        assert_eq!(ran(&mut host, "-sk big"), wrote("2 big\n"));
        assert_eq!(ran(&mut host, "-sk e"), wrote("total 0\n"));
    }

    #[test]
    fn f_lists_as_found_and_q_shows_what_does_not_print() {
        let mut host = Host::new("found", &["only"]);
        assert_eq!(ran(&mut host, "-f"), wrote(".\n..\nonly\n"));
        std::fs::write(host.root.join("a\tb"), "").unwrap();
        assert_eq!(ran(&mut host, "-q a\tb"), wrote("a?b\n"));
        assert_eq!(ran(&mut host, "a\tb"), wrote("a\tb\n"));
    }

    #[test]
    fn what_cannot_be_read_is_said_and_the_rest_listed() {
        let mut host = Host::new("missing", &["a", "d/x"]);
        assert_eq!(
            ran(&mut host, "nope a d"),
            (
                false,
                "a\n\nd:\nx\n".into(),
                "ls: nope: no such file\n".into()
            )
        );
        let mut entries = Vec::new();
        for (name, size) in [("b", 1), ("a", 1), ("c", 2)] {
            entries.push(Entry {
                name: name.into(),
                found: Found {
                    directory: false,
                    size,
                },
            });
        }
        let given = words("-S");
        let taken = asked(&given).unwrap();
        let names: Vec<String> = ordered(entries, &taken)
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["c", "a", "b"], "a tie of sizes goes by name");
        assert!(
            shows(".x", Hidden::All)
                && shows(".x", Hidden::AlmostAll)
                && !shows(".x", Hidden::None)
        );
        assert!(!shows("..", Hidden::AlmostAll) && shows("..", Hidden::All));
        assert_eq!(joined("a/", "b"), "a/b");
        assert_eq!(joined("a", "b"), "a/b");
    }

    #[test]
    fn options_go_together() {
        let mut host = Host::new("together", &["x/a", "x/bb", "x/s/c", "f", "gg"]);
        assert_eq!(ran(&mut host, "-pR x"), wrote("x:\na\nbb\ns/\n\nx/s:\nc\n"));
        assert_eq!(
            ran(&mut host, "-rd x f gg"),
            wrote("x\ngg\nf\n"),
            "-d with -r reverses them all"
        );
        assert_eq!(
            ran(&mut host, "-Sr gg f"),
            wrote("f\ngg\n"),
            "files too are ordered by size"
        );
        assert_eq!(
            ran(&mut host, "-sR x"),
            wrote("x:\ntotal 2\n1 a\n1 bb\n0 s\n\nx/s:\ntotal 1\n1 c\n")
        );
        assert_eq!(ran(&mut host, "-m x"), wrote("a, bb, s\n"));
        assert_eq!(
            ran(&mut host, "-1 x/"),
            wrote("a\nbb\ns\n"),
            "a trailing slash names the directory"
        );
        assert_eq!(ran(&mut host, "-dp x/s"), wrote("x/s/\n"));
        assert_eq!(ran(&mut host, "-ap x/s"), wrote("./\n../\nc\n"));
    }

    #[test]
    fn an_empty_directory_lists_nothing_but_its_title() {
        let mut host = Host::new("empty", &["e/", "f/x"]);
        assert_eq!(ran(&mut host, "e"), wrote(""));
        assert_eq!(ran(&mut host, "e f"), wrote("e:\n\nf:\nx\n"));
        assert_eq!(ran(&mut host, "-C e"), wrote(""));
        assert_eq!(ran(&mut host, "-m e"), wrote(""));
        assert_eq!(ran(&mut host, "-A e"), wrote(""));
    }

    #[test]
    fn a_directory_that_cannot_be_read_is_said_and_the_rest_walked() {
        use std::os::unix::fs::PermissionsExt as _;
        let mut host = Host::new("shut", &["t/a/x", "t/b/y"]);
        let shut = host.root.join("t/a");
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o000)).unwrap();
        let readable = std::fs::read_dir(&shut).is_ok();
        let found = ran(&mut host, "-R t");
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o755)).unwrap();
        if readable {
            // One who may read anything reads it all the same.
            assert_eq!(found, wrote("t:\na\nb\n\nt/a:\nx\n\nt/b:\ny\n"));
        } else {
            assert_eq!(
                found,
                (
                    false,
                    "t:\na\nb\n\nt/b:\ny\n".into(),
                    "ls: t/a: could not be read\n".into()
                )
            );
        }
    }

    #[test]
    fn names_with_blanks_are_written_as_they_are() {
        let mut host = Host::new("blanks", &["a b", "c"]);
        assert_eq!(ran(&mut host, ""), wrote("a b\nc\n"));
        assert_eq!(ran(&mut host, "-m"), wrote("a b, c\n"));
        let entry = Entry {
            name: "d\x07e".into(),
            found: Found {
                directory: true,
                size: 0,
            },
        };
        let given = words("-qFs");
        let taken = asked(&given).unwrap();
        assert_eq!(shown(&entry, &taken), "0 d?e/");
        let given = words("");
        let taken = asked(&given).unwrap();
        assert_eq!(shown(&entry, &taken), "d\x07e");
    }

    #[test]
    fn a_file_named_twice_is_written_twice() {
        let mut host = Host::new("twice", &["a", "d/x"]);
        assert_eq!(ran(&mut host, "a a"), wrote("a\na\n"));
        assert_eq!(ran(&mut host, "d d"), wrote("d:\nx\n\nd:\nx\n"));
        assert_eq!(
            ran(&mut host, "nope nope"),
            (
                false,
                String::new(),
                "ls: nope: no such file\nls: nope: no such file\n".into()
            )
        );
    }
}
