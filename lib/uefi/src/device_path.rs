//! Device paths (UEFI 2.10, chapter 10): a chain of nodes that says where
//! something is, ending in an end node. The loader takes the path of the
//! partition it was started from and adds a file to it, to start another
//! system's loader from the same partition.

/// The node that ends a whole device path.
const END: [u8; 4] = [0x7f, 0xff, 4, 0];
const MEDIA: u8 = 4;
const HARD_DRIVE: u8 = 1;
const FILE_PATH: u8 = 4;

/// Whether a node header is the end of the whole path. An end of one
/// instance, 7f 01, is not: more instances follow it.
#[must_use]
pub fn is_end(header: [u8; 4]) -> bool {
    header[0] == END[0] && header[1] == END[1]
}

/// The nodes of `path` before its end node, or `None` when a node's
/// length points past the bytes or is shorter than a node header.
#[must_use]
pub fn nodes(path: &[u8]) -> Option<&[u8]> {
    let mut at = 0;
    loop {
        let header: [u8; 4] = path.get(at..at + 4)?.try_into().ok()?;
        if is_end(header) {
            return Some(&path[..at]);
        }
        let len = usize::from(u16::from_le_bytes([header[2], header[3]]));
        if len < 4 {
            return None;
        }
        at = at.checked_add(len)?;
    }
}

/// One node of a device path: its type, its subtype and the bytes after
/// its four byte header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Node<'a> {
    pub kind: u8,
    pub subtype: u8,
    pub data: &'a [u8],
}

/// The nodes of a whole device path, before its end node.
#[derive(Clone, Debug)]
pub struct Nodes<'a>(&'a [u8]);

impl<'a> Iterator for Nodes<'a> {
    type Item = Node<'a>;

    fn next(&mut self) -> Option<Node<'a>> {
        let header = self.0.get(..4)?;
        let len = usize::from(u16::from_le_bytes([header[2], header[3]]));
        let node = self.0.get(..len)?;
        self.0 = &self.0[len..];
        Some(Node {
            kind: header[0],
            subtype: header[1],
            data: node.get(4..)?,
        })
    }
}

/// The nodes of `path`, or `None` when it is broken as [`nodes`] says.
#[must_use]
pub fn walk(path: &[u8]) -> Option<Nodes<'_>> {
    nodes(path).map(Nodes)
}

/// The file `path` names, as the text of its file path nodes one after the
/// other, since a firmware may split a name into several. `None` when the
/// path is broken or names no file, as a whole disk or a firmware
/// application does.
#[must_use]
pub fn file(path: &[u8]) -> Option<impl Iterator<Item = char> + '_> {
    let mut files = walk(path)?
        .filter(|n| n.kind == MEDIA && n.subtype == FILE_PATH)
        .peekable();
    files.peek()?;
    let units = files.flat_map(|n| {
        n.data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&u| u16::from_le_bytes(u))
            .take_while(|&u| u != 0)
    });
    Some(char::decode_utf16(units).map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER)))
}

/// The signature of the partition `path` lies on: a GPT partition's GUID,
/// or an MBR disk's four byte signature followed by zeros.
#[must_use]
pub fn partition(path: &[u8]) -> Option<[u8; 16]> {
    hard_drive(path)?.get(20..36)?.try_into().ok()
}

/// The number of the partition `path` lies on, counted from 1 as the
/// partition table counts its entries.
#[must_use]
pub fn partition_number(path: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(
        hard_drive(path)?.get(..4)?.try_into().ok()?,
    ))
}

/// What the hard drive node of `path` holds after its header.
fn hard_drive(path: &[u8]) -> Option<&[u8]> {
    walk(path)?
        .find(|n| n.kind == MEDIA && n.subtype == HARD_DRIVE)
        .map(|n| n.data)
}

/// `path` from its hard drive node on, end node included: the short form
/// that starts at the partition, which the firmware completes by finding
/// the partition by its signature (UEFI 2.10, section 3.1.2). `None` when
/// the path is broken or names no partition.
#[must_use]
pub fn from_partition(path: &[u8]) -> Option<&[u8]> {
    let end = nodes(path)?.len() + END.len();
    let mut at = 0;
    for node in walk(path)? {
        if node.kind == MEDIA && node.subtype == HARD_DRIVE {
            return path.get(at..end);
        }
        at += 4 + node.data.len();
    }
    None
}

/// Writes `base`, a whole device path, with a file path node for `file`
/// before its end, into `out`. `/` in `file` becomes `\`. Returns the
/// length of the new path, or `None` when `base` is broken, the file name
/// holds what UCS-2 cannot, or `out` is too small.
#[must_use]
pub fn with_file(base: &[u8], file: &str, out: &mut [u8]) -> Option<usize> {
    let head = nodes(base)?;
    if file.contains('\0') {
        return None;
    }
    let units = file.chars().count() + 1;
    let node_len = 4 + 2 * units;
    let total = head.len() + node_len + END.len();
    if total > out.len() {
        return None;
    }
    out[..head.len()].copy_from_slice(head);
    let node = &mut out[head.len()..head.len() + node_len];
    node[..2].copy_from_slice(&[MEDIA, FILE_PATH]);
    node[2..4].copy_from_slice(&u16::try_from(node_len).ok()?.to_le_bytes());
    for (slot, c) in node[4..]
        .as_chunks_mut::<2>()
        .0
        .iter_mut()
        .zip(file.chars().chain(core::iter::once('\0')))
    {
        let unit = match c {
            '/' => u16::from(b'\\'),
            c => u16::try_from(u32::from(c)).ok()?,
        };
        slot.copy_from_slice(&unit.to_le_bytes());
    }
    out[total - END.len()..total].copy_from_slice(&END);
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device path like OVMF gives a partition: PCI, SATA, then a hard
    /// drive node, then the end.
    fn sample_partition() -> Vec<u8> {
        let mut p = vec![0x02, 0x01, 0x0c, 0x00, 0xd0, 0x41, 0x03, 0x0a, 0, 0, 0, 0];
        p.extend([0x01, 0x01, 0x06, 0x00, 0x00, 0x1f]);
        p.extend([0x03, 0x12, 0x0a, 0x00, 0, 0, 0xff, 0xff, 0, 0]);
        let mut hd = vec![0x04, 0x01, 0x2a, 0x00];
        hd.extend([0x5a; 0x2a - 4]);
        p.extend(hd);
        p.extend(END);
        p
    }

    #[test]
    fn nodes_end_before_the_end_node() {
        let p = sample_partition();
        assert_eq!(nodes(&p), Some(&p[..p.len() - 4]));
        assert_eq!(nodes(&END), Some(&[][..]), "an empty path");
    }

    #[test]
    fn only_7f_ff_ends_a_path() {
        assert!(is_end([0x7f, 0xff, 4, 0]));
        assert!(!is_end([0x7f, 0x01, 4, 0]), "the end of an instance");
        assert!(!is_end([0x04, 0xff, 4, 0]));
        assert!(!is_end([0x00, 0x00, 0, 0]));
    }

    #[test]
    fn an_end_of_instance_is_not_the_end() {
        // Two instances: the first ends with 7f 01, the whole path with 7f ff.
        let mut p = vec![0x01, 0x01, 0x06, 0x00, 0xaa, 0xbb];
        p.extend([0x7f, 0x01, 0x04, 0x00]);
        p.extend([0x01, 0x01, 0x06, 0x00, 0xcc, 0xdd]);
        p.extend(END);
        assert_eq!(nodes(&p).map(<[u8]>::len), Some(16));
    }

    #[test]
    fn a_name_too_long_for_one_node_is_refused() {
        let long = "a".repeat(40_000);
        let mut out = vec![0u8; 100_000];
        assert_eq!(
            with_file(&END, &long, &mut out),
            None,
            "a node's length is 16 bits"
        );
    }

    #[test]
    fn broken_paths_are_refused() {
        let p = sample_partition();
        assert_eq!(nodes(&p[..p.len() - 4]), None, "no end node");
        assert_eq!(
            nodes(&[0x01, 0x01, 0x02, 0x00]),
            None,
            "a node shorter than its header"
        );
        assert_eq!(
            nodes(&[0x01, 0x01, 0x40, 0x00, 0, 0]),
            None,
            "a node past the end"
        );
        assert_eq!(nodes(&[]), None);
        assert_eq!(nodes(&[0x7f, 0xff]), None, "a cut end node");
    }

    #[test]
    fn a_file_is_added_before_the_end() {
        let p = sample_partition();
        let mut out = [0u8; 256];
        let n = with_file(&p, "/EFI/Microsoft/Boot/bootmgfw.efi", &mut out).unwrap();
        let head = p.len() - 4;
        assert_eq!(&out[..head], &p[..head], "the partition's nodes stay");
        let file = &out[head..n - 4];
        assert_eq!(&file[..2], [MEDIA, FILE_PATH]);
        let len = usize::from(u16::from_le_bytes([file[2], file[3]]));
        assert_eq!(len, file.len());
        let units: Vec<u16> = file[4..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| u16::from_le_bytes(b))
            .collect();
        let want: Vec<u16> = "\\EFI\\Microsoft\\Boot\\bootmgfw.efi\0"
            .encode_utf16()
            .collect();
        assert_eq!(units, want);
        assert_eq!(&out[n - 4..n], END);
        assert_eq!(
            nodes(&out[..n]).map(<[u8]>::len),
            Some(n - 4),
            "the result reads back"
        );
    }

    #[test]
    fn every_instance_is_kept_and_the_file_goes_last() {
        let mut p = vec![0x01, 0x01, 0x06, 0x00, 0xaa, 0xbb];
        p.extend([0x7f, 0x01, 0x04, 0x00]);
        p.extend([0x01, 0x01, 0x06, 0x00, 0xcc, 0xdd]);
        p.extend(END);
        let mut out = [0u8; 64];
        let n = with_file(&p, "x", &mut out).unwrap();
        assert_eq!(
            &out[..16],
            &p[..16],
            "both instances and the end of the first"
        );
        assert_eq!(&out[16..20], [MEDIA, FILE_PATH, 8, 0]);
        assert_eq!(&out[20..24], [b'x', 0, 0, 0]);
        assert_eq!(&out[24..n], END);
    }

    #[test]
    fn an_empty_name_is_a_lone_terminator() {
        let mut out = [0u8; 16];
        assert_eq!(with_file(&END, "", &mut out), Some(10));
        assert_eq!(&out[..10], [MEDIA, FILE_PATH, 6, 0, 0, 0, 0x7f, 0xff, 4, 0]);
    }

    #[test]
    fn the_output_has_to_hold_everything() {
        let p = sample_partition();
        let need = p.len() + 4 + 2 * 4;
        let mut exact = vec![0u8; need];
        assert_eq!(with_file(&p, "a.e", &mut exact), Some(need));
        let mut short = vec![0u8; need - 1];
        assert_eq!(with_file(&p, "a.e", &mut short), None);
    }

    #[test]
    fn names_ucs2_cannot_hold_are_refused() {
        let mut out = [0u8; 128];
        assert_eq!(with_file(&END, "\u{1F600}.efi", &mut out), None);
        assert_eq!(
            with_file(&END, "a\0b", &mut out),
            None,
            "a NUL inside the name"
        );
        assert_eq!(with_file(&[0x01], "a", &mut out), None, "a broken base");
        assert_eq!(with_file(&END, "caf\u{e9}", &mut out), Some(4 + 10 + 4));
    }

    fn file_node(text: &str) -> Vec<u8> {
        let units: Vec<u16> = text.encode_utf16().chain([0]).collect();
        let mut n = vec![MEDIA, FILE_PATH];
        n.extend(u16::try_from(4 + 2 * units.len()).unwrap().to_le_bytes());
        n.extend(units.iter().flat_map(|u| u.to_le_bytes()));
        n
    }

    fn text(path: &[u8]) -> Option<String> {
        file(path).map(Iterator::collect)
    }

    #[test]
    fn the_file_comes_from_the_file_path_nodes() {
        let mut out = vec![0u8; 512];
        let len = with_file(
            &sample_partition(),
            "/EFI/Microsoft/Boot/bootmgfw.efi",
            &mut out,
        )
        .unwrap();
        assert_eq!(
            text(&out[..len]).as_deref(),
            Some("\\EFI\\Microsoft\\Boot\\bootmgfw.efi")
        );
        let mut split = sample_partition();
        split.truncate(split.len() - 4);
        split.extend(file_node("\\EFI\\ubuntu"));
        split.extend(file_node("\\shimx64.efi"));
        split.extend(END);
        assert_eq!(
            text(&split).as_deref(),
            Some("\\EFI\\ubuntu\\shimx64.efi"),
            "names split over nodes join"
        );
        assert_eq!(text(&sample_partition()), None, "a partition names no file");
        assert_eq!(text(&END), None);
        assert_eq!(text(&sample_partition()[..10]), None, "a broken path");
    }

    #[test]
    fn a_file_name_stops_at_its_zero_and_survives_bad_units() {
        let mut p = file_node("ab");
        let len = p.len();
        p[len - 2..].copy_from_slice(&0xd800u16.to_le_bytes());
        p.extend(END);
        assert_eq!(
            text(&p).as_deref(),
            Some("ab\u{fffd}"),
            "a lone surrogate, and no zero"
        );
        let mut q = file_node("a\0b");
        q.extend(END);
        assert_eq!(text(&q).as_deref(), Some("a"));
    }

    #[test]
    fn the_partition_is_its_hard_drive_node_signature() {
        assert_eq!(partition(&sample_partition()), Some([0x5a; 16]));
        let mut gpt = vec![MEDIA, HARD_DRIVE, 0x2a, 0x00];
        let mut data = vec![0u8; 0x2a - 4];
        data[20..36].copy_from_slice(&[7; 16]);
        data[36] = 2;
        data[37] = 2;
        gpt.extend(data);
        gpt.extend(END);
        assert_eq!(partition(&gpt), Some([7; 16]));
        let mut short = vec![MEDIA, HARD_DRIVE, 0x10, 0x00];
        short.extend([0; 12]);
        short.extend(END);
        assert_eq!(partition(&short), None, "a node too short to hold one");
        let mut vendor_first = vec![MEDIA, 3, 44, 0];
        vendor_first.extend([9; 40]);
        vendor_first.extend(&gpt);
        assert_eq!(
            partition(&vendor_first),
            Some([7; 16]),
            "another media node before the hard drive node is skipped"
        );
        assert_eq!(partition(&END), None);
    }

    #[test]
    fn the_partition_number_leads_its_node() {
        assert_eq!(partition_number(&sample_partition()), Some(0x5a5a_5a5a));
        let mut second = vec![MEDIA, HARD_DRIVE, 0x2a, 0x00];
        let mut data = vec![0xee; 0x2a - 4];
        data[..4].copy_from_slice(&2u32.to_le_bytes());
        second.extend(data);
        second.extend(END);
        assert_eq!(partition_number(&second), Some(2));
        let mut pci_first = vec![0x01, 0x01, 0x06, 0x00, 0x00, 0x1f];
        pci_first.extend(&second);
        assert_eq!(partition_number(&pci_first), Some(2));
        let mut short = vec![MEDIA, HARD_DRIVE, 0x07, 0x00, 1, 0, 0];
        short.extend(END);
        assert_eq!(
            partition_number(&short),
            None,
            "a node too short to hold one"
        );
        assert_eq!(partition_number(&END), None);
    }

    #[test]
    fn the_short_form_starts_at_the_partition() {
        let p = sample_partition();
        let mut whole = [0u8; 256];
        let n = with_file(&p, "/EFI/tool.efi", &mut whole).unwrap();
        let whole = &whole[..n];
        let hd = 12 + 6 + 10;
        let short = from_partition(whole).unwrap();
        assert_eq!(
            short,
            &whole[hd..],
            "the hard drive node, the file and the end"
        );
        assert_eq!(partition(short), partition(whole));
        assert_eq!(text(short).as_deref(), Some("\\EFI\\tool.efi"));
        assert_eq!(from_partition(short), Some(short), "already short");
        assert_eq!(
            from_partition(&p),
            Some(&p[hd..]),
            "a partition without a file"
        );
    }

    #[test]
    fn without_a_partition_there_is_no_short_form() {
        let mut disk = vec![0x02, 0x01, 0x0c, 0x00, 0xd0, 0x41, 0x03, 0x0a, 0, 0, 0, 0];
        disk.extend(END);
        assert_eq!(from_partition(&disk), None);
        assert_eq!(from_partition(&END), None);
        // A CD's boot image is a media node too, but not a hard drive.
        let mut cd = vec![0x04, 0x02, 0x18, 0x00];
        cd.extend([0; 0x18 - 4]);
        cd.extend(END);
        assert_eq!(from_partition(&cd), None);
        let mut broken = sample_partition();
        broken.truncate(broken.len() - 4);
        assert_eq!(from_partition(&broken), None, "no end node");
    }

    #[test]
    fn walking_gives_each_node() {
        let p = sample_partition();
        let kinds: Vec<(u8, u8, usize)> = walk(&p)
            .unwrap()
            .map(|n| (n.kind, n.subtype, n.data.len()))
            .collect();
        assert_eq!(kinds, [(2, 1, 8), (1, 1, 2), (3, 0x12, 6), (4, 1, 0x26)]);
        assert_eq!(walk(&END).unwrap().count(), 0);
        assert!(walk(&p[..p.len() - 4]).is_none());
    }
}
