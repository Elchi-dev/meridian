//! The programs the loader hands the kernel: the files in
//! \meridian\programs on the EFI system partition.

use core::fmt::Write;

use bootinfo::Module;
use canvas::Text;

/// The name a file of the folder is handed over with, or none when it is
/// no program: a folder, a hidden file such as the ones macOS leaves on
/// FAT disks, or a name longer than a module keeps.
pub fn name_of(
    name: impl IntoIterator<Item = char>,
    directory: bool,
) -> Option<Text<{ Module::NAME }>> {
    if directory {
        return None;
    }
    let mut text = Text::new();
    for c in name {
        text.write_char(c).ok()?;
    }
    let s = text.as_str();
    (!s.is_empty() && !s.starts_with('.')).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(s: &str, directory: bool) -> Option<String> {
        name_of(s.chars(), directory).map(|t| t.as_str().to_string())
    }

    #[test]
    fn files_are_handed_over_by_name() {
        assert_eq!(name("hello", false).as_deref(), Some("hello"));
        assert_eq!(name("init.elf", false).as_deref(), Some("init.elf"));
        assert_eq!(name("Hello World", false).as_deref(), Some("Hello World"));
    }

    #[test]
    fn folders_hidden_files_and_nothing_are_no_programs() {
        assert_eq!(name("tools", true), None);
        assert_eq!(name(".", true), None);
        assert_eq!(name("..", true), None);
        assert_eq!(
            name("._hello", false),
            None,
            "what macOS leaves next to a file"
        );
        assert_eq!(name(".DS_Store", false), None);
        assert_eq!(name("", false), None);
    }

    #[test]
    fn a_name_has_to_fit_into_a_module() {
        let longest = "a".repeat(Module::NAME);
        assert_eq!(name(&longest, false), Some(longest.clone()));
        assert_eq!(name(&format!("{longest}b"), false), None);
        let wide = "\u{e9}".repeat(Module::NAME / 2);
        assert_eq!(name(&wide, false), Some(wide.clone()), "two bytes each");
        assert_eq!(name(&format!("{wide}x"), false), None);
        let name = name_of(longest.chars(), false).unwrap();
        assert!(Module::new(0, 1, name.as_str().as_bytes()).is_some());
    }
}
