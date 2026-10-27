//! The paths a pattern for names of files matches, for both dialects: its
//! parts between slashes, each a name as written or a test for the names
//! in the directory it stands in, walked through a tree of files.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

/// What the shell asks of a tree of files to match patterns in it.
pub trait Tree {
    /// The names in the directory at `path`, none when it is no directory.
    fn names(&mut self, path: &str) -> Vec<String>;

    /// Whether something is at `path`.
    fn has(&mut self, path: &str) -> bool;
}

/// A part of a pattern between two slashes.
pub enum Part<'t> {
    /// A name as written, which is not listed for, since a directory such
    /// as the root holds places that are no names in it.
    Name(String),
    /// What each name in the directory is put to.
    Test(Box<dyn Fn(&str) -> bool + 't>),
}

/// The paths `parts` lead to in `tree`, from the root when `absolute` and
/// from `directory` otherwise, each written as the pattern starts it. The
/// names a test takes in one directory come in the order `order` puts
/// them. A path that ends in a name as written counts once something is
/// there.
pub fn walk(
    directory: &str,
    absolute: bool,
    parts: &[Part<'_>],
    order: impl Fn(&str, &str) -> Ordering,
    tree: &mut impl Tree,
) -> Vec<String> {
    let base = if absolute { "/" } else { directory };
    // Each path found so far: as the pattern writes it, and from the root.
    let mut found = Vec::from([(String::new(), String::from(base))]);
    let mut listed_last = true;
    for part in parts {
        let mut next = Vec::new();
        for (shown, real) in &found {
            let names = match part {
                Part::Name(name) => Vec::from([name.clone()]),
                Part::Test(test) => {
                    let mut names = tree.names(real);
                    names.retain(|name| test(name));
                    names.sort_by(|a, b| order(a, b));
                    names
                }
            };
            for name in names {
                let shown = if shown.is_empty() && !absolute {
                    name.clone()
                } else {
                    format!("{shown}/{name}")
                };
                let slash = if real.ends_with('/') { "" } else { "/" };
                next.push((shown, format!("{real}{slash}{name}")));
            }
        }
        listed_last = matches!(part, Part::Test(_));
        found = next;
    }
    found
        .into_iter()
        .filter(|(_, real)| listed_last || tree.has(real))
        .map(|(shown, _)| shown)
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use alloc::vec;
    use std::path::PathBuf;

    /// Directories and files on the host, in a directory of their own
    /// among its temporary files, for patterns to match against; gone once
    /// dropped. Paths go from the tree's root and are found in capitals,
    /// as Meridian's tree of files finds them, and it keeps every path it
    /// was asked to list or to find. It lists names the other way round
    /// from any order a test wants, so nothing leans on how a disk happens
    /// to keep them.
    #[derive(Debug)]
    pub(crate) struct Host {
        root: PathBuf,
        pub listed: Vec<String>,
        pub asked: Vec<String>,
    }

    impl Host {
        /// The tree that holds `paths`, those that end in `/` directories.
        pub(crate) fn new(name: &str, paths: &[&str]) -> Host {
            let root =
                std::env::temp_dir().join(format!("meridian-walk-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            for path in paths {
                let at = root.join(path.trim_matches('/'));
                if path.ends_with('/') {
                    std::fs::create_dir_all(at).unwrap();
                } else {
                    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
                    std::fs::write(at, b"").unwrap();
                }
            }
            Host {
                root,
                listed: Vec::new(),
                asked: Vec::new(),
            }
        }

        /// Makes the file `name`, right under the tree's root, hold `bytes`.
        pub(crate) fn put(&self, name: &str, bytes: &[u8]) {
            std::fs::write(self.root.join(name), bytes).unwrap();
        }

        /// What the file at `path` holds, none when nothing readable is
        /// there.
        pub(crate) fn read(&self, path: &str) -> Option<Vec<u8>> {
            std::fs::read(self.find(path)?).ok()
        }

        /// Where `path` is on the host, each name found in capitals, `.`
        /// staying and `..` going up as Meridian's paths do; none when
        /// nothing is there or the path does not start at the root.
        fn find(&self, path: &str) -> Option<PathBuf> {
            let mut names = Vec::new();
            for name in path.strip_prefix('/')?.split('/') {
                match name {
                    "" | "." => {}
                    ".." => {
                        names.pop()?;
                    }
                    name => names.push(name),
                }
            }
            let mut at = self.root.clone();
            for name in names {
                let entry = std::fs::read_dir(&at).ok()?.find(|e| {
                    e.as_ref().is_ok_and(|e| {
                        e.file_name().to_string_lossy().to_uppercase() == name.to_uppercase()
                    })
                })?;
                at = entry.ok()?.path();
            }
            Some(at)
        }
    }

    impl Tree for Host {
        fn names(&mut self, path: &str) -> Vec<String> {
            self.listed.push(path.into());
            let mut names: Vec<String> = self
                .find(path)
                .and_then(|at| std::fs::read_dir(at).ok())
                .map(|dir| {
                    dir.map(|e| e.unwrap().file_name().into_string().unwrap())
                        .collect()
                })
                .unwrap_or_default();
            names.sort_by(|a, b| b.cmp(a));
            names
        }

        fn has(&mut self, path: &str) -> bool {
            self.asked.push(path.into());
            self.find(path).is_some()
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// `pattern`'s parts, each with a `*` a test of whether a name starts
    /// with what comes before it, the others names as written.
    fn parts(pattern: &str) -> Vec<Part<'static>> {
        pattern
            .split('/')
            .filter(|p| !p.is_empty())
            .map(|p| match p.split_once('*') {
                Some((start, _)) => {
                    let start = String::from(start);
                    Part::Test(Box::new(move |name: &str| name.starts_with(&start)))
                }
                None => Part::Name(p.into()),
            })
            .collect()
    }

    fn walked(tree: &mut Host, directory: &str, pattern: &str) -> Vec<String> {
        let absolute = pattern.starts_with('/');
        walk(directory, absolute, &parts(pattern), str::cmp, tree)
    }

    #[test]
    fn a_pattern_leads_part_by_part() {
        let mut tree = Host::new("lead", &["a/x", "a/y/", "b/x", "ab/z/x", "c"]);
        assert_eq!(walked(&mut tree, "/", "/*/x"), ["/a/x", "/b/x"]);
        assert_eq!(walked(&mut tree, "/", "/a*"), ["/a", "/ab"]);
        assert_eq!(walked(&mut tree, "/", "/a*/*/x"), ["/ab/z/x"]);
        assert_eq!(
            walked(&mut tree, "/a", "*"),
            ["x", "y"],
            "from the directory"
        );
        assert_eq!(walked(&mut tree, "/a/", "*"), ["x", "y"]);
        assert_eq!(walked(&mut tree, "/a", "/b/*"), ["/b/x"], "from the root");
        assert_eq!(
            walked(&mut tree, "/", "/c/*"),
            Vec::<String>::new(),
            "a file holds none"
        );
        assert_eq!(walked(&mut tree, "/", "/nope/*"), Vec::<String>::new());
    }

    #[test]
    fn names_come_in_the_order_asked_for() {
        let mut tree = Host::new("order", &["b", "A", "c"]);
        let all = parts("/*");
        assert_eq!(
            walk("/", true, &all, str::cmp, &mut tree),
            ["/A", "/b", "/c"]
        );
        let backwards = |a: &str, b: &str| b.cmp(a);
        assert_eq!(
            walk("/", true, &all, backwards, &mut tree),
            ["/c", "/b", "/A"]
        );
    }

    #[test]
    fn a_name_as_written_counts_once_something_is_there() {
        let mut tree = Host::new("written", &["d/x", "e/y"]);
        assert_eq!(
            walked(&mut tree, "/", "/*/X"),
            ["/d/X"],
            "found in capitals, kept as written"
        );
        assert_eq!(tree.asked, ["/d/X", "/e/X"]);
        assert_eq!(walked(&mut tree, "/", "/*/nope"), Vec::<String>::new());
        tree.asked.clear();
        assert_eq!(walked(&mut tree, "/", "/d/*"), ["/d/x"]);
        assert_eq!(
            tree.asked,
            Vec::<String>::new(),
            "what a listing found is there"
        );
    }

    #[test]
    fn only_the_directories_a_test_is_in_are_listed() {
        let mut tree = Host::new("listed", &["s/b/a.m", "s/b/b.t"]);
        assert_eq!(walked(&mut tree, "/", "/s/b/a*"), ["/s/b/a.m"]);
        assert_eq!(
            tree.listed,
            ["/s/b"],
            "not the root, which holds mounts, nor /s"
        );
        tree.listed.clear();
        assert_eq!(walked(&mut tree, "/", "/s*/b/a*"), ["/s/b/a.m"]);
        assert_eq!(tree.listed, ["/", "/s/b"]);
        tree.listed.clear();
        assert_eq!(walked(&mut tree, "/s", "b/b*"), ["b/b.t"]);
        assert_eq!(tree.listed, ["/s/b"]);
    }

    #[test]
    fn a_test_is_put_to_each_name_in_its_directory() {
        let mut tree = Host::new("seen", &["q/one", "q/two"]);
        let seen = core::cell::RefCell::new(Vec::new());
        let test = Part::Test(Box::new(|name: &str| {
            seen.borrow_mut().push(String::from(name));
            name.len() == 3
        }));
        let found = walk(
            "/",
            true,
            &[Part::Name("q".into()), test],
            str::cmp,
            &mut tree,
        );
        assert_eq!(found, ["/q/one", "/q/two"]);
        let mut seen = seen.into_inner();
        seen.sort();
        assert_eq!(seen, vec![String::from("one"), String::from("two")]);
    }
}
