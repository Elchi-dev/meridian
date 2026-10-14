// Package fat writes FAT32 volumes for EFI System Partitions, following
// Microsoft's "FAT: General Overview of On-Disk Format", version 1.03
// (fatgen103).
//
// The volume is written into a region of a larger image. Clusters that no
// file uses are not cleared, so callers that want reproducible images write
// into a fresh, zero-filled file.
package fat

import (
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"slices"
	"strings"
	"time"
	"unicode/utf16"
)

const (
	sectorSize      = 512
	reservedSectors = 32
	numFATs         = 2
	dirEntrySize    = 32
	lfnChars        = 13
	maxLongName     = 255
	maxDirEntries   = 65536

	// The smallest volume fatgen103's cluster size table accepts for FAT32;
	// anything smaller would end up with fewer than 65525 clusters.
	minSectors      = 66601
	minFAT32Cluster = 65525

	endOfChain = 0x0FFFFFFF
	mediaFixed = 0xF8

	attrVolumeID  = 0x08
	attrDirectory = 0x10
	attrArchive   = 0x20
	attrLongName  = 0x0F
	lastLFNFlag   = 0x40
)

// Volume describes the FAT32 file system to write.
type Volume struct {
	// Size of the volume in bytes, a multiple of 512.
	Size int64
	// Label is stored in upper case; up to 11 characters. Empty means none.
	Label string
	// ID is the volume serial number.
	ID uint32
	// HiddenSectors is the LBA where the volume starts on its disk.
	HiddenSectors uint32
	// Time is used for every timestamp, so the same input gives the same bytes.
	Time time.Time
	// Files maps slash-separated paths such as "EFI/BOOT/BOOTX64.EFI" to
	// their contents. Directories are created as needed.
	Files map[string][]byte
}

// Result describes the volume that was written.
type Result struct {
	SectorsPerCluster uint32
	SectorsPerFAT     uint32
	Clusters          uint32
	UsedClusters      uint32
}

type node struct {
	name     string
	dir      bool
	data     []byte
	children []*node
	index    map[string]*node
	parent   *node

	shortName [11]byte
	longName  bool
	first     uint32
	clusters  uint32
}

type geometry struct {
	totalSectors      uint32
	sectorsPerCluster uint32
	fatSectors        uint32
	clusters          uint32
}

func (g geometry) clusterBytes() int64 { return int64(g.sectorsPerCluster) * sectorSize }

func (g geometry) clusterOffset(c uint32) int64 {
	dataStart := int64(reservedSectors+numFATs*g.fatSectors) * sectorSize
	return dataStart + int64(c-2)*g.clusterBytes()
}

// Write formats a FAT32 volume of v.Size bytes at offset in w and fills it
// with v.Files.
func Write(w io.WriterAt, offset int64, v Volume) (Result, error) {
	g, err := planGeometry(v.Size)
	if err != nil {
		return Result{}, err
	}
	label, err := volumeLabel(v.Label)
	if err != nil {
		return Result{}, err
	}
	stamp, err := encodeTime(v.Time)
	if err != nil {
		return Result{}, err
	}
	root, err := buildTree(v.Files)
	if err != nil {
		return Result{}, err
	}
	assignShortNames(root)
	used, err := allocate(root, g, label)
	if err != nil {
		return Result{}, err
	}
	if used > g.clusters {
		missing := int64(used-g.clusters) * g.clusterBytes()
		return Result{}, fmt.Errorf("fat: the files need %d clusters of %d bytes but the volume has %d; it is %d bytes (%d KiB) short",
			used, g.clusterBytes(), g.clusters, missing, (missing+1023)/1024)
	}

	fs := &writer{w: w, offset: offset, g: g}
	reserved := make([]byte, reservedSectors*sectorSize)
	boot, info := bootSector(g, v, label), fsInfo(g, used)
	copy(reserved[0:], boot)
	copy(reserved[sectorSize:], info)
	// fatgen103 section 3.6 recommends the backup boot record at sector 6.
	copy(reserved[6*sectorSize:], boot)
	copy(reserved[7*sectorSize:], info)
	fs.put(0, reserved)
	fat := fatTable(root, g)
	for i := range uint32(numFATs) {
		fs.put(reservedSectors+i*g.fatSectors, fat)
	}
	fs.writeTree(root, label, stamp)
	if fs.err != nil {
		return Result{}, fs.err
	}
	return Result{
		SectorsPerCluster: g.sectorsPerCluster,
		SectorsPerFAT:     g.fatSectors,
		Clusters:          g.clusters,
		UsedClusters:      used,
	}, nil
}

// planGeometry picks the cluster size from the table in fatgen103 section 3.5
// and computes the FAT size with the formula given there.
func planGeometry(size int64) (geometry, error) {
	if size <= 0 || size%sectorSize != 0 {
		return geometry{}, fmt.Errorf("fat: volume size %d is not a positive multiple of %d bytes", size, sectorSize)
	}
	if size/sectorSize > 0xFFFFFFFF {
		return geometry{}, fmt.Errorf("fat: volume size %d is above the FAT32 limit of 2 TiB", size)
	}
	total := uint32(size / sectorSize)
	if total < minSectors {
		return geometry{}, fmt.Errorf("fat: a FAT32 volume needs at least %d bytes (%d sectors), this one has %d", minSectors*sectorSize, minSectors, size)
	}
	var spc uint32
	switch {
	case total <= 532480:
		spc = 1
	case total <= 16777216:
		spc = 8
	case total <= 33554432:
		spc = 16
	case total <= 67108864:
		spc = 32
	default:
		spc = 64
	}
	// Done in 64 bits: near 2 TiB the rounding step overflows 32.
	tmp1 := uint64(total) - reservedSectors
	tmp2 := uint64(256*spc+numFATs) / 2
	fatSectors := uint32((tmp1 + tmp2 - 1) / tmp2)
	clusters := (total - reservedSectors - numFATs*fatSectors) / spc
	return geometry{totalSectors: total, sectorsPerCluster: spc, fatSectors: fatSectors, clusters: clusters}, nil
}

func volumeLabel(label string) ([11]byte, error) {
	var out [11]byte
	label = strings.ToUpper(label)
	if label == "" {
		return padded("NO NAME"), nil
	}
	if len(label) > 11 {
		return out, fmt.Errorf("fat: label %q is longer than 11 characters", label)
	}
	for _, r := range label {
		if r != ' ' && !shortNameChar(r) {
			return out, fmt.Errorf("fat: label %q contains %q, which a FAT label cannot hold", label, r)
		}
	}
	return padded(label), nil
}

// padded lays text into the 11 bytes of a name field, filled with spaces.
func padded(text string) [11]byte {
	field := [11]byte{' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' ', ' '}
	copy(field[:], text)
	return field
}

// shortName lays out an 8.3 name: the base in the first eight bytes and the
// extension in the last three, each padded with spaces.
func shortName(base, ext string) [11]byte {
	field := padded("")
	copy(field[:8], base)
	copy(field[8:], ext)
	return field
}

type timestamp struct {
	date, time uint16
	tenths     byte
}

// encodeTime packs a time into the FAT date and time fields from fatgen103
// section 6.
func encodeTime(t time.Time) (timestamp, error) {
	if t.Year() < 1980 || t.Year() > 2107 {
		return timestamp{}, fmt.Errorf("fat: timestamps must be between 1980 and 2107, got %s", t.Format(time.RFC3339))
	}
	return timestamp{
		date:   uint16(t.Year()-1980)<<9 | uint16(t.Month())<<5 | uint16(t.Day()),
		time:   uint16(t.Hour())<<11 | uint16(t.Minute())<<5 | uint16(t.Second()/2),
		tenths: byte(t.Second()%2*100 + t.Nanosecond()/10_000_000),
	}, nil
}

func buildTree(files map[string][]byte) (*node, error) {
	root := &node{index: map[string]*node{}}
	paths := make([]string, 0, len(files))
	for p := range files {
		paths = append(paths, p)
	}
	slices.Sort(paths)
	for _, p := range paths {
		parts := strings.Split(p, "/")
		cur := root
		for i, part := range parts {
			if err := checkLongName(part); err != nil {
				return nil, fmt.Errorf("fat: path %q: %w", p, err)
			}
			last := i == len(parts)-1
			child := cur.child(part)
			if child != nil && child.name != part {
				return nil, fmt.Errorf("fat: %q and %q differ only in case, which FAT cannot tell apart", child.path(), p)
			}
			// Paths are sorted, so a file always arrives before any path that
			// would put something inside it.
			if child != nil && !child.dir {
				return nil, fmt.Errorf("fat: %q is both a file and a directory", child.path())
			}
			if child == nil {
				child = &node{name: part, dir: !last, parent: cur}
				if last {
					child.data = files[p]
				} else {
					child.index = map[string]*node{}
				}
				cur.children = append(cur.children, child)
				cur.index[strings.ToUpper(part)] = child
			}
			cur = child
		}
		if err := checkFileSize(p, int64(len(cur.data))); err != nil {
			return nil, err
		}
	}
	return root, nil
}

func checkFileSize(path string, n int64) error {
	if n > 0xFFFFFFFF {
		return fmt.Errorf("fat: %q is %d bytes, FAT32 files end at 4 GiB", path, n)
	}
	return nil
}

func (n *node) child(name string) *node {
	return n.index[strings.ToUpper(name)]
}

func (n *node) path() string {
	if n.parent == nil || n.parent.parent == nil {
		return n.name
	}
	return n.parent.path() + "/" + n.name
}

// checkLongName applies the rules for long names in fatgen103 section 7.
func checkLongName(name string) error {
	if name == "" {
		return errors.New("empty path component")
	}
	if name == "." || name == ".." {
		return fmt.Errorf("%q is not allowed as a name", name)
	}
	if len(utf16.Encode([]rune(name))) > maxLongName {
		return fmt.Errorf("name %q is longer than %d characters", name, maxLongName)
	}
	for _, r := range name {
		if r < 0x20 || strings.ContainsRune(`"*/:<>?\|`, r) {
			return fmt.Errorf("name %q contains %q, which FAT does not allow", name, r)
		}
	}
	if strings.HasSuffix(name, ".") || strings.HasSuffix(name, " ") {
		return fmt.Errorf("name %q ends in a period or space, which FAT drops", name)
	}
	return nil
}

func shortNameChar(r rune) bool {
	return r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' || strings.ContainsRune("$%'-_@~`!(){}^#&", r)
}

// plainShortName returns the 8.3 form of a name that is already a valid
// upper-case short name, and false for anything that needs a long name.
func plainShortName(name string) ([11]byte, bool) {
	base, ext, hasDot := strings.Cut(name, ".")
	if base == "" || len(base) > 8 || len(ext) > 3 || hasDot && ext == "" {
		return [11]byte{}, false
	}
	for _, r := range base + ext {
		if !shortNameChar(r) {
			return [11]byte{}, false
		}
	}
	return shortName(base, ext), true
}

// basisName follows the basis-name generation steps in fatgen103 section 7:
// upper case, invalid characters become underscores, spaces and leading
// periods go, the base is what comes before the first period, cut to 8
// characters, and the extension is what follows the last period, cut to 3.
func basisName(name string) (base, ext string) {
	clean := func(s string) string {
		var b strings.Builder
		for _, r := range strings.ToUpper(s) {
			switch {
			case r == ' ':
			case shortNameChar(r):
				b.WriteRune(r)
			default:
				b.WriteByte('_')
			}
		}
		return b.String()
	}
	base, rest, hasDot := strings.Cut(strings.TrimLeft(name, ". "), ".")
	if hasDot {
		ext = rest[strings.LastIndex(rest, ".")+1:]
	}
	base, ext = clean(base), clean(ext)
	if base == "" {
		base = "_"
	}
	return base[:min(len(base), 8)], ext[:min(len(ext), 3)]
}

func assignShortNames(dir *node) {
	taken := map[[11]byte]bool{}
	var generated []*node
	for _, c := range dir.children {
		if sn, ok := plainShortName(c.name); ok {
			c.shortName = sn
			taken[sn] = true
		} else {
			c.longName = true
			generated = append(generated, c)
		}
	}
	// A directory holds at most 65536 entries, so a free tail always turns up
	// long before fatgen103's limit of ~999999.
	for _, c := range generated {
		base, ext := basisName(c.name)
		for n := 1; ; n++ {
			tail := fmt.Sprintf("~%d", n)
			b := base[:min(len(base), 8-len(tail))] + tail
			sn := shortName(b, ext)
			if !taken[sn] {
				c.shortName, taken[sn] = sn, true
				break
			}
		}
	}
	for _, c := range dir.children {
		if c.dir {
			assignShortNames(c)
		}
	}
}

func lfnEntryCount(name string) int {
	return (len(utf16.Encode([]rune(name))) + lfnChars - 1) / lfnChars
}

func clustersFor(bytes int64, g geometry) uint32 {
	return uint32((bytes + g.clusterBytes() - 1) / g.clusterBytes())
}

// allocate hands out clusters in one pass, depth first, so every file and
// directory is contiguous. It returns how many clusters are in use. A
// directory's size comes from encoding its entries, the same code that later
// writes them, so the two can never disagree.
func allocate(root *node, g geometry, label [11]byte) (uint32, error) {
	next := uint32(2)
	var walk func(d *node) error
	walk = func(d *node) error {
		size := len(encodeDir(d, label, timestamp{}))
		if size/dirEntrySize > maxDirEntries {
			return fmt.Errorf("fat: directory %q needs %d entries, FAT allows %d", "/"+d.path(), size/dirEntrySize, maxDirEntries)
		}
		d.first = next
		d.clusters = max(clustersFor(int64(size), g), 1)
		next += d.clusters
		for _, c := range d.children {
			if c.dir {
				if err := walk(c); err != nil {
					return err
				}
			} else if len(c.data) > 0 {
				c.first = next
				c.clusters = clustersFor(int64(len(c.data)), g)
				next += c.clusters
			}
		}
		return nil
	}
	if err := walk(root); err != nil {
		return 0, err
	}
	return next - 2, nil
}

func fatTable(root *node, g geometry) []byte {
	fat := make([]byte, int64(g.fatSectors)*sectorSize)
	binary.LittleEndian.PutUint32(fat[0:], 0x0FFFFF00|mediaFixed)
	binary.LittleEndian.PutUint32(fat[4:], endOfChain)
	var walk func(n *node)
	walk = func(n *node) {
		for i := range n.clusters {
			c := n.first + i
			next := uint32(endOfChain)
			if i+1 < n.clusters {
				next = c + 1
			}
			binary.LittleEndian.PutUint32(fat[c*4:], next)
		}
		for _, child := range n.children {
			walk(child)
		}
	}
	walk(root)
	return fat
}

func bootSector(g geometry, v Volume, label [11]byte) []byte {
	b := make([]byte, sectorSize)
	copy(b[0:3], []byte{0xEB, 0x58, 0x90})
	// fatgen103 recommends MSWIN4.1 as the OEM name because it is the one
	// least likely to upset older drivers.
	copy(b[3:11], "MSWIN4.1")
	binary.LittleEndian.PutUint16(b[11:], sectorSize)
	b[13] = byte(g.sectorsPerCluster)
	binary.LittleEndian.PutUint16(b[14:], reservedSectors)
	b[16] = numFATs
	b[21] = mediaFixed
	binary.LittleEndian.PutUint16(b[24:], 63)
	binary.LittleEndian.PutUint16(b[26:], 255)
	binary.LittleEndian.PutUint32(b[28:], v.HiddenSectors)
	binary.LittleEndian.PutUint32(b[32:], g.totalSectors)
	binary.LittleEndian.PutUint32(b[36:], g.fatSectors)
	binary.LittleEndian.PutUint32(b[44:], 2)
	binary.LittleEndian.PutUint16(b[48:], 1)
	binary.LittleEndian.PutUint16(b[50:], 6)
	b[64] = 0x80
	b[66] = 0x29
	binary.LittleEndian.PutUint32(b[67:], v.ID)
	copy(b[71:82], label[:])
	copy(b[82:90], "FAT32   ")
	b[510], b[511] = 0x55, 0xAA
	return b
}

// fsInfo builds the FSInfo sector from fatgen103 section 5.
func fsInfo(g geometry, used uint32) []byte {
	b := make([]byte, sectorSize)
	binary.LittleEndian.PutUint32(b[0:], 0x41615252)
	binary.LittleEndian.PutUint32(b[484:], 0x61417272)
	binary.LittleEndian.PutUint32(b[488:], g.clusters-used)
	nextFree := uint32(0xFFFFFFFF)
	if used < g.clusters {
		nextFree = 2 + used
	}
	binary.LittleEndian.PutUint32(b[492:], nextFree)
	binary.LittleEndian.PutUint32(b[508:], 0xAA550000)
	return b
}

type writer struct {
	w      io.WriterAt
	offset int64
	g      geometry
	err    error
}

func (fs *writer) at(off int64, data []byte) {
	if fs.err != nil {
		return
	}
	at := fs.offset + off
	if _, err := fs.w.WriteAt(data, at); err != nil {
		fs.err = fmt.Errorf("fat: write at byte %d: %w", at, err)
	}
}

func (fs *writer) put(sector uint32, data []byte) {
	fs.at(int64(sector)*sectorSize, data)
}

func (fs *writer) writeTree(dir *node, label [11]byte, ts timestamp) {
	buf := make([]byte, int64(dir.clusters)*fs.g.clusterBytes())
	copy(buf, encodeDir(dir, label, ts))
	fs.at(fs.g.clusterOffset(dir.first), buf)
	for _, c := range dir.children {
		if c.dir {
			fs.writeTree(c, label, ts)
		} else {
			// The whole cluster run is written, so the tail of the last
			// cluster holds zeros and not whatever the disk held before.
			data := make([]byte, int64(c.clusters)*fs.g.clusterBytes())
			copy(data, c.data)
			fs.at(fs.g.clusterOffset(c.first), data)
		}
	}
}

func encodeDir(dir *node, label [11]byte, ts timestamp) []byte {
	var entries []byte
	if dir.parent == nil {
		if string(label[:]) != "NO NAME    " {
			entries = append(entries, shortEntry(label, attrVolumeID, 0, 0, ts)...)
		}
	} else {
		parentFirst := dir.parent.first
		if dir.parent.parent == nil {
			parentFirst = 0
		}
		entries = append(entries, shortEntry(dotName("."), attrDirectory, dir.first, 0, ts)...)
		entries = append(entries, shortEntry(dotName(".."), attrDirectory, parentFirst, 0, ts)...)
	}
	for _, c := range dir.children {
		if c.longName {
			entries = append(entries, longEntries(c.name, c.shortName)...)
		}
		attr, size := byte(attrArchive), uint32(len(c.data))
		if c.dir {
			attr, size = attrDirectory, 0
		}
		entries = append(entries, shortEntry(c.shortName, attr, c.first, size, ts)...)
	}
	return entries
}

func dotName(dots string) [11]byte {
	return padded(dots)
}

// shortEntry encodes a directory entry from fatgen103 section 6.
func shortEntry(name [11]byte, attr byte, first, size uint32, ts timestamp) []byte {
	e := make([]byte, dirEntrySize)
	copy(e[0:11], name[:])
	e[11] = attr
	if attr != attrVolumeID {
		e[13] = ts.tenths
		binary.LittleEndian.PutUint16(e[14:], ts.time)
		binary.LittleEndian.PutUint16(e[16:], ts.date)
		binary.LittleEndian.PutUint16(e[18:], ts.date)
	}
	binary.LittleEndian.PutUint16(e[20:], uint16(first>>16))
	binary.LittleEndian.PutUint16(e[22:], ts.time)
	binary.LittleEndian.PutUint16(e[24:], ts.date)
	binary.LittleEndian.PutUint16(e[26:], uint16(first))
	binary.LittleEndian.PutUint32(e[28:], size)
	return e
}

// shortNameChecksum is the rotate-and-add sum from fatgen103 section 7 that
// ties long name entries to their short entry.
func shortNameChecksum(sn [11]byte) byte {
	var sum byte
	for _, c := range sn {
		sum = (sum>>1 | sum<<7) + c
	}
	return sum
}

// longEntries returns the long name entries for a name in the order they
// are stored: the last part first, flagged with 0x40.
func longEntries(name string, sn [11]byte) []byte {
	units := utf16.Encode([]rune(name))
	count := lfnEntryCount(name)
	padded := make([]uint16, count*lfnChars)
	for i := range padded {
		switch {
		case i < len(units):
			padded[i] = units[i]
		case i == len(units):
			padded[i] = 0x0000
		default:
			padded[i] = 0xFFFF
		}
	}
	sum := shortNameChecksum(sn)
	var out []byte
	for seq := count; seq >= 1; seq-- {
		e := make([]byte, dirEntrySize)
		e[0] = byte(seq)
		if seq == count {
			e[0] |= lastLFNFlag
		}
		e[11] = attrLongName
		e[13] = sum
		chunk := padded[(seq-1)*lfnChars : seq*lfnChars]
		for i, u := range chunk {
			var off int
			switch {
			case i < 5:
				off = 1 + 2*i
			case i < 11:
				off = 14 + 2*(i-5)
			default:
				off = 28 + 2*(i-11)
			}
			binary.LittleEndian.PutUint16(e[off:], u)
		}
		out = append(out, e...)
	}
	return out
}
