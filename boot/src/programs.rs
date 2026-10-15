//! The programs the loader hands the kernel: the files in
//! \meridian\programs on the EFI system partition.

use core::fmt::Write;
use core::mem::size_of;

use bootinfo::{Module, Modules};
use canvas::Text;

/// The modules the loader hands over, written one after another into the
/// page that holds them for the kernel, at most [`Modules::MAX`].
pub struct List<'a> {
    page: &'a mut [u8],
    count: usize,
}

impl<'a> List<'a> {
    pub fn new(page: &'a mut [u8]) -> List<'a> {
        List { page, count: 0 }
    }

    /// Whether no other module fits.
    #[must_use]
    pub fn full(&self) -> bool {
        self.count == Modules::MAX || (self.count + 1) * size_of::<Module>() > self.page.len()
    }

    /// Adds `module` unless the list is full, and says whether it did.
    pub fn push(&mut self, module: &Module) -> bool {
        if self.full() {
            return false;
        }
        let at = self.count * size_of::<Module>();
        self.page[at..at + size_of::<Module>()].copy_from_slice(&module.to_bytes());
        self.count += 1;
        true
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.count
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

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

    fn module(i: u64) -> Module {
        let name = format!("program-{i}");
        Module::new(0x10_0000 * (i + 1), 0x1000 + i, name.as_bytes()).unwrap()
    }

    #[test]
    fn modules_lie_one_after_another() {
        let mut page = vec![0u8; 4096];
        let mut list = List::new(&mut page);
        assert!(list.is_empty() && !list.full());
        assert!(list.push(&module(0)) && list.push(&module(1)) && list.push(&module(2)));
        assert_eq!(list.len(), 3);
        let size = size_of::<Module>();
        for i in 0..3 {
            let bytes = page[i * size..(i + 1) * size].try_into().unwrap();
            assert_eq!(Module::from_bytes(bytes), module(i as u64), "module {i}");
        }
        assert!(
            page[3 * size..].iter().all(|&b| b == 0),
            "nothing past them"
        );
    }

    #[test]
    fn the_list_holds_no_more_than_the_kernel_reads() {
        let mut page = vec![0u8; 4096];
        let mut list = List::new(&mut page);
        for i in 0..Modules::MAX as u64 {
            assert!(!list.full(), "room for module {i}");
            assert!(list.push(&module(i)));
        }
        assert!(list.full());
        assert!(!list.push(&module(99)), "one more is refused");
        assert_eq!(list.len(), Modules::MAX);
        assert!(
            page[Modules::MAX * size_of::<Module>()..]
                .iter()
                .all(|&b| b == 0),
            "and leaves the page alone"
        );
    }

    #[test]
    fn the_list_holds_no_more_than_its_page() {
        let mut small = vec![0u8; 2 * size_of::<Module>() + 10];
        let mut list = List::new(&mut small);
        assert!(list.push(&module(0)) && list.push(&module(1)));
        assert!(list.full(), "two fit");
        assert!(!list.push(&module(2)));
        assert_eq!(list.len(), 2);
        let mut none = [0u8; 47];
        assert!(List::new(&mut none).full(), "not even one");
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
