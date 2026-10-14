// Package iso writes ISO 9660 images (ECMA-119, 4th edition) that boot UEFI
// firmware through an El Torito boot catalog (El Torito specification 1.0)
// whose single entry points at an embedded FAT image.
package iso

import (
	"bytes"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"slices"
	"strings"
	"time"
)

const (
	blockSize = 2048

	systemAreaBlocks = 16
	pvdBlock         = 16
	bootRecordBlock  = 17
	terminatorBlock  = 18
	catalogBlock     = 19
	pathTableBlock   = 20

	// Interchange level 2 (ECMA-119 section 10.2): up to 30 characters for a
	// file name and extension together, 31 for a directory, 8 levels deep.
	maxFileIDChars = 30
	maxDirIDChars  = 31
	maxDepth       = 8
	maxPathBytes   = 255

	// The platform ID UEFI firmware looks for in the boot catalog (UEFI
	// specification section 13.3.2.1).
	platformEFI = 0xEF

	flagDirectory = 0x02
)

// Image describes the ISO to write.
type Image struct {
	// VolumeID is the label, up to 32 upper-case letters, digits and
	// underscores.
	VolumeID string
	// Time is used for every timestamp, so the same input gives the same bytes.
	Time time.Time
	// Files appear in the ISO 9660 tree. Names are stored in upper case and
	// must otherwise follow interchange level 2.
	Files map[string][]byte
	// BootImage is the FAT image the firmware boots from.
	BootImage []byte
}

type node struct {
	id       string
	key      string
	path     string
	dir      bool
	data     []byte
	children []*node
	parent   *node

	number int
	lba    uint32
	size   uint32
}

// Write lays out the image and writes it into w from offset 0. It returns
// the size of the image in bytes.
func Write(w io.WriterAt, img Image) (int64, error) {
	volID, err := volumeID(img.VolumeID)
	if err != nil {
		return 0, err
	}
	if img.Time.Year() < 1900 || img.Time.Year() > 2155 {
		return 0, fmt.Errorf("iso: timestamps must be between 1900 and 2155, got %s", img.Time.Format(time.RFC3339))
	}
	if len(img.BootImage) == 0 {
		return 0, errors.New("iso: there is no boot image; the firmware would have nothing to start")
	}
	if len(img.BootImage)%512 != 0 {
		return 0, fmt.Errorf("iso: the boot image is %d bytes, not a whole number of 512-byte sectors", len(img.BootImage))
	}
	root, err := buildTree(img.Files)
	if err != nil {
		return 0, err
	}
	dirs := pathTableOrder(root)
	pathTable := encodePathTable(dirs, binary.LittleEndian)
	ptBlocks := blocks(len(pathTable))

	next := uint32(pathTableBlock + 2*ptBlocks)
	for _, d := range dirs {
		d.size = uint32(blocks(len(encodeDir(d, [7]byte{}))) * blockSize)
		d.lba = next
		next += d.size / blockSize
	}
	for _, f := range filesInOrder(root) {
		f.size = uint32(len(f.data))
		if f.size > 0 {
			f.lba = next
			next += uint32(blocks(len(f.data)))
		}
	}
	bootLBA := next
	total := bootLBA + uint32(blocks(len(img.BootImage)))

	ts := encodeDirTime(img.Time)
	out := &writer{w: w}
	out.block(0, make([]byte, systemAreaBlocks*blockSize))
	out.block(pvdBlock, primaryDescriptor(volID, total, root, len(pathTable), ptBlocks, img.Time, ts))
	out.block(bootRecordBlock, bootRecord())
	out.block(terminatorBlock, terminator())
	out.block(catalogBlock, bootCatalog(bootLBA, len(img.BootImage)))
	out.block(pathTableBlock, pad(encodePathTable(dirs, binary.LittleEndian)))
	out.block(pathTableBlock+uint32(ptBlocks), pad(encodePathTable(dirs, binary.BigEndian)))
	for _, d := range dirs {
		out.block(d.lba, pad(encodeDir(d, ts)))
	}
	for _, f := range filesInOrder(root) {
		out.block(f.lba, pad(f.data))
	}
	out.block(bootLBA, pad(img.BootImage))
	if out.err != nil {
		return 0, out.err
	}
	return int64(total) * blockSize, nil
}

type writer struct {
	w   io.WriterAt
	err error
}

func (o *writer) block(lba uint32, data []byte) {
	if o.err != nil {
		return
	}
	if _, err := o.w.WriteAt(data, int64(lba)*blockSize); err != nil {
		o.err = fmt.Errorf("iso: write at block %d: %w", lba, err)
	}
}

func blocks(n int) int { return (n + blockSize - 1) / blockSize }

func pad(b []byte) []byte {
	out := make([]byte, blocks(len(b))*blockSize)
	copy(out, b)
	return out
}

func dChar(r rune) bool {
	return r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' || r == '_'
}

func volumeID(id string) (string, error) {
	if len(id) > 32 {
		return "", fmt.Errorf("iso: volume ID %q is longer than 32 characters", id)
	}
	for _, r := range id {
		if !dChar(r) {
			return "", fmt.Errorf("iso: volume ID %q contains %q; only A to Z, 0 to 9 and _ are allowed", id, r)
		}
	}
	return id, nil
}

// fileID turns a name into its ISO 9660 identifier: upper case, with ";1"
// as the version for files.
func fileID(name string, dir bool) (string, error) {
	upper := strings.ToUpper(name)
	base, ext, hasDot := strings.Cut(upper, ".")
	if dir {
		if hasDot {
			return "", fmt.Errorf("directory name %q contains a period, which ISO 9660 does not allow", name)
		}
		if len(base) > maxDirIDChars {
			return "", fmt.Errorf("directory name %q is longer than %d characters", name, maxDirIDChars)
		}
	} else if len(base)+len(ext) > maxFileIDChars {
		return "", fmt.Errorf("file name %q is longer than %d characters", name, maxFileIDChars)
	}
	if base == "" && ext == "" {
		return "", fmt.Errorf("name %q is empty", name)
	}
	if strings.Contains(ext, ".") {
		return "", fmt.Errorf("file name %q has more than one period", name)
	}
	for _, r := range base + ext {
		if !dChar(r) {
			return "", fmt.Errorf("name %q contains %q; ISO 9660 names allow only A to Z, 0 to 9 and _", name, r)
		}
	}
	if dir {
		return base, nil
	}
	return base + "." + ext + ";1", nil
}

func buildTree(files map[string][]byte) (*node, error) {
	root := &node{}
	paths := make([]string, 0, len(files))
	for p := range files {
		paths = append(paths, p)
	}
	slices.Sort(paths)
	for _, p := range paths {
		parts := strings.Split(p, "/")
		if len(parts) > maxDepth {
			return nil, fmt.Errorf("iso: %q is %d levels deep, ISO 9660 allows %d", p, len(parts), maxDepth)
		}
		cur, pathBytes := root, len(parts)-1
		for i, part := range parts {
			last := i == len(parts)-1
			id, err := fileID(part, !last)
			if err != nil {
				return nil, fmt.Errorf("iso: path %q: %w", p, err)
			}
			pathBytes += len(id)
			key := strings.ToUpper(part)
			var child *node
			for _, c := range cur.children {
				if c.key == key {
					child = c
				}
			}
			prefix := strings.Join(parts[:i+1], "/")
			if child != nil && child.path != prefix {
				return nil, fmt.Errorf("iso: %q and %q differ only in case, which ISO 9660 cannot tell apart", child.path, prefix)
			}
			// Paths are sorted, so a file always arrives before any path that
			// would put something inside it.
			if child != nil && !child.dir {
				return nil, fmt.Errorf("iso: %q is both a file and a directory", prefix)
			}
			if child == nil {
				child = &node{id: id, key: key, path: prefix, dir: !last, parent: cur}
				if last {
					child.data = files[p]
				}
				cur.children = append(cur.children, child)
			}
			cur = child
		}
		if pathBytes > maxPathBytes {
			return nil, fmt.Errorf("iso: %q is %d bytes long as an ISO 9660 path, the limit is %d", p, pathBytes, maxPathBytes)
		}
	}
	sortTree(root)
	return root, nil
}

// sortTree orders every directory as ECMA-119 section 9.3 asks: by name,
// then by extension.
func sortTree(n *node) {
	slices.SortFunc(n.children, func(a, b *node) int { return compareIDs(a.id, b.id) })
	for _, c := range n.children {
		if c.dir {
			sortTree(c)
		}
	}
}

// compareIDs gives the order of section 9.3: name, then extension, the
// shorter one padded with spaces. Every character allowed in a name sorts
// after the space and after the period, so comparing "NAME.EXT" as one string
// gives that same order. The version has to go first, though: ";" sorts after
// the digits and would put "A.B1;1" before "A.B;1".
func compareIDs(a, b string) int {
	a, _, _ = strings.Cut(a, ";")
	b, _, _ = strings.Cut(b, ";")
	return strings.Compare(a, b)
}

// pathTableOrder numbers the directories breadth first, which gives the
// order section 9.4 requires: by level, then by parent number, then by name.
func pathTableOrder(root *node) []*node {
	root.number = 1
	order := []*node{root}
	for i := 0; i < len(order); i++ {
		for _, c := range order[i].children {
			if c.dir {
				c.number = len(order) + 1
				order = append(order, c)
			}
		}
	}
	return order
}

func filesInOrder(root *node) []*node {
	var out []*node
	var walk func(n *node)
	walk = func(n *node) {
		for _, c := range n.children {
			if c.dir {
				walk(c)
			} else {
				out = append(out, c)
			}
		}
	}
	walk(root)
	return out
}

func encodePathTable(dirs []*node, order binary.ByteOrder) []byte {
	var out []byte
	for _, d := range dirs {
		id := []byte{0}
		parent := 1
		if d.parent != nil {
			id, parent = []byte(d.id), d.parent.number
		}
		rec := make([]byte, 8)
		rec[0] = byte(len(id))
		order.PutUint32(rec[2:], d.lba)
		order.PutUint16(rec[6:], uint16(parent))
		rec = append(rec, id...)
		if len(id)%2 == 1 {
			rec = append(rec, 0)
		}
		out = append(out, rec...)
	}
	return out
}

// encodeDir lays out a directory's records, starting a new block whenever
// a record would cross one, as section 6.8.1.1 demands. Sizing and writing
// both use it, so they cannot disagree.
func encodeDir(d *node, ts [7]byte) []byte {
	var out []byte
	put := func(rec []byte) {
		if room := blockSize - len(out)%blockSize; len(rec) > room {
			out = append(out, make([]byte, room)...)
		}
		out = append(out, rec...)
	}
	parent := d
	if d.parent != nil {
		parent = d.parent
	}
	put(dirRecord("\x00", d.lba, d.size, flagDirectory, ts))
	put(dirRecord("\x01", parent.lba, parent.size, flagDirectory, ts))
	for _, c := range d.children {
		flags := byte(0)
		if c.dir {
			flags = flagDirectory
		}
		put(dirRecord(c.id, c.lba, c.size, flags, ts))
	}
	return out
}

func putBoth32(b []byte, v uint32) {
	binary.LittleEndian.PutUint32(b[0:], v)
	binary.BigEndian.PutUint32(b[4:], v)
}

func putBoth16(b []byte, v uint16) {
	binary.LittleEndian.PutUint16(b[0:], v)
	binary.BigEndian.PutUint16(b[2:], v)
}

// dirRecord encodes a directory record from ECMA-119 section 9.1; an even
// identifier length gets one padding byte.
func dirRecord(id string, lba, size uint32, flags byte, ts [7]byte) []byte {
	rec := make([]byte, 33+len(id)+(1-len(id)%2))
	rec[0] = byte(len(rec))
	putBoth32(rec[2:], lba)
	putBoth32(rec[10:], size)
	copy(rec[18:25], ts[:])
	rec[25] = flags
	putBoth16(rec[28:], 1)
	rec[32] = byte(len(id))
	copy(rec[33:], id)
	return rec
}

// encodeDirTime packs a time into the seven bytes of section 9.1.5, in UTC.
func encodeDirTime(t time.Time) [7]byte {
	u := t.UTC()
	return [7]byte{byte(u.Year() - 1900), byte(u.Month()), byte(u.Day()), byte(u.Hour()), byte(u.Minute()), byte(u.Second()), 0}
}

// volumeTime is the 17-byte form of section 8.4.26.1, in UTC.
func volumeTime(t time.Time) []byte {
	u := t.UTC()
	s := fmt.Sprintf("%04d%02d%02d%02d%02d%02d%02d", u.Year(), u.Month(), u.Day(), u.Hour(), u.Minute(), u.Second(), u.Nanosecond()/10_000_000)
	return append([]byte(s), 0)
}

// padded is s in a field of n bytes, filled with spaces.
func padded(s string, n int) []byte {
	field := bytes.Repeat([]byte{' '}, n)
	copy(field, s)
	return field
}

// primaryDescriptor encodes the primary volume descriptor of section 8.4.
func primaryDescriptor(volID string, total uint32, root *node, ptSize, ptBlocks int, t time.Time, ts [7]byte) []byte {
	b := make([]byte, blockSize)
	b[0] = 1
	copy(b[1:6], "CD001")
	b[6] = 1
	copy(b[8:40], padded("", 32))
	copy(b[40:72], padded(volID, 32))
	putBoth32(b[80:], total)
	putBoth16(b[120:], 1)
	putBoth16(b[124:], 1)
	putBoth16(b[128:], blockSize)
	putBoth32(b[132:], uint32(ptSize))
	binary.LittleEndian.PutUint32(b[140:], pathTableBlock)
	binary.BigEndian.PutUint32(b[148:], uint32(pathTableBlock+ptBlocks))
	copy(b[156:190], dirRecord("\x00", root.lba, root.size, flagDirectory, ts))
	copy(b[190:318], padded("", 128))
	copy(b[318:446], padded("", 128))
	copy(b[446:574], padded("", 128))
	copy(b[574:702], padded("SEXTANT", 128))
	copy(b[702:813], padded("", 111))
	copy(b[813:830], volumeTime(t))
	copy(b[830:847], volumeTime(t))
	copy(b[847:864], append([]byte(strings.Repeat("0", 16)), 0))
	copy(b[864:881], append([]byte(strings.Repeat("0", 16)), 0))
	b[881] = 1
	return b
}

// bootRecord is the El Torito boot record volume descriptor (section 2.0).
func bootRecord() []byte {
	b := make([]byte, blockSize)
	copy(b[1:6], "CD001")
	b[6] = 1
	copy(b[7:39], "EL TORITO SPECIFICATION")
	binary.LittleEndian.PutUint32(b[71:], catalogBlock)
	return b
}

func terminator() []byte {
	b := make([]byte, blockSize)
	b[0] = 255
	copy(b[1:6], "CD001")
	b[6] = 1
	return b
}

// bootCatalog holds the validation entry and the default entry (El Torito
// sections 2.1 and 2.2). The sector count is in 512-byte units and only 16
// bits wide; a larger image gets 0, which EDK2 and most firmware read as
// "the rest of the disc". That is why the boot image is always written last.
func bootCatalog(bootLBA uint32, bootSize int) []byte {
	b := make([]byte, blockSize)
	v := b[0:32]
	v[0] = 0x01
	v[1] = platformEFI
	copy(v[4:28], "MERIDIAN")
	v[30], v[31] = 0x55, 0xAA
	var sum uint16
	for i := 0; i < 32; i += 2 {
		sum += binary.LittleEndian.Uint16(v[i:])
	}
	binary.LittleEndian.PutUint16(v[28:], -sum)

	d := b[32:64]
	d[0] = 0x88
	count := bootSize / 512
	if count > 0xFFFF {
		count = 0
	}
	binary.LittleEndian.PutUint16(d[6:], uint16(count))
	binary.LittleEndian.PutUint32(d[8:], bootLBA)
	return b
}
