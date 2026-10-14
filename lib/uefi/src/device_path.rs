//! Device paths (UEFI 2.10, chapter 10): a chain of nodes that says where
//! something is, ending in an end node. The loader takes the path of the
//! partition it was started from and adds a file to it, to start another
//! system's loader from the same partition.

/// The node that ends a whole device path.
const END: [u8; 4] = [0x7f, 0xff, 4, 0];
const MEDIA: u8 = 4;
const FILE_PATH: u8 = 4;

/// The nodes of `path` before its end node, or `None` when a node's
/// length points past the bytes or is shorter than a node header.
#[must_use]
pub fn nodes(path: &[u8]) -> Option<&[u8]> {
    let mut at = 0;
    loop {
        let header = path.get(at..at + 4)?;
        if header[0] == END[0] && header[1] == END[1] {
            return Some(&path[..at]);
        }
        let len = usize::from(u16::from_le_bytes([header[2], header[3]]));
        if len < 4 {
            return None;
        }
        at = at.checked_add(len)?;
    }
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
    fn partition() -> Vec<u8> {
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
        let p = partition();
        assert_eq!(nodes(&p), Some(&p[..p.len() - 4]));
        assert_eq!(nodes(&END), Some(&[][..]), "an empty path");
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
        let p = partition();
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
        let p = partition();
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
    fn the_output_has_to_hold_everything() {
        let p = partition();
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
}
