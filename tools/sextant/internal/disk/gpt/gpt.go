// Package gpt writes GUID partition tables as described in chapter 5 of the
// UEFI specification (version 2.10).
package gpt

import (
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"fmt"
	"hash/crc32"
	"io"
	"strings"
	"unicode/utf16"
)

// SectorSize is the logical block size every image is written with.
const SectorSize = 512

const (
	// Partitions start on 1 MiB boundaries, like every current partitioning
	// tool does, which keeps them aligned on 4K-sector drives too.
	alignSectors = (1 << 20) / SectorSize

	entryCount   = 128
	entrySize    = 128
	entrySectors = entryCount * entrySize / SectorSize
	headerSize   = 92
	nameUnits    = 36

	// LBA 0 holds the protective MBR, LBA 1 the header, then the entry array.
	firstUsableLBA = 2 + entrySectors
	// The backup entry array and backup header take the last sectors.
	backupSectors = entrySectors + 1
)

// GUID is a globally unique identifier in its on-disk byte order: the first
// three fields are little-endian, the last two are stored as written.
type GUID [16]byte

// EFISystemPartition is the partition type firmware looks for when booting.
var EFISystemPartition = mustParseGUID("C12A7328-F81F-11D2-BA4B-00A0C93EC93B")

// MeridianSystem marks the partition that holds Meridian's own file system.
var MeridianSystem = mustParseGUID("B0DD29AB-2286-43EF-9809-E345D01A1B29")

// BasicData is the type Windows gives its data partitions, which other
// systems use for theirs too.
var BasicData = mustParseGUID("EBD0A0A2-B9E5-4433-87C0-68B6B72699C7")

// ParseGUID reads the usual text form, such as
// C12A7328-F81F-11D2-BA4B-00A0C93EC93B, in upper or lower case.
func ParseGUID(s string) (GUID, error) {
	var g GUID
	parts := strings.Split(s, "-")
	if len(parts) != 5 || len(parts[0]) != 8 || len(parts[1]) != 4 || len(parts[2]) != 4 || len(parts[3]) != 4 || len(parts[4]) != 12 {
		return g, fmt.Errorf("gpt: %q is not a GUID; expected the form C12A7328-F81F-11D2-BA4B-00A0C93EC93B", s)
	}
	canonical, err := hex.DecodeString(strings.Join(parts, ""))
	if err != nil {
		return g, fmt.Errorf("gpt: %q is not a GUID: it contains a character that is not hexadecimal", s)
	}
	return fromCanonical([16]byte(canonical)), nil
}

func mustParseGUID(s string) GUID {
	g, err := ParseGUID(s)
	if err != nil {
		panic(err)
	}
	return g
}

func fromCanonical(c [16]byte) GUID {
	var g GUID
	g[0], g[1], g[2], g[3] = c[3], c[2], c[1], c[0]
	g[4], g[5] = c[5], c[4]
	g[6], g[7] = c[7], c[6]
	copy(g[8:], c[8:])
	return g
}

func (g GUID) canonical() [16]byte {
	var c [16]byte
	c[0], c[1], c[2], c[3] = g[3], g[2], g[1], g[0]
	c[4], c[5] = g[5], g[4]
	c[6], c[7] = g[7], g[6]
	copy(c[8:], g[8:])
	return c
}

func (g GUID) String() string {
	c := g.canonical()
	h := strings.ToUpper(hex.EncodeToString(c[:]))
	return h[0:8] + "-" + h[8:12] + "-" + h[12:16] + "-" + h[16:20] + "-" + h[20:32]
}

// deriveGUID turns the image seed into a version 4 GUID. The same seed and
// label always give the same GUID, so an image can be rebuilt byte for byte.
func deriveGUID(seed, label string) GUID {
	sum := sha256.Sum256([]byte(seed + "\x00" + label))
	var c [16]byte
	copy(c[:], sum[:16])
	c[6] = c[6]&0x0f | 0x40
	c[8] = c[8]&0x3f | 0x80
	return fromCanonical(c)
}

// Partition describes one partition to create.
type Partition struct {
	Name       string
	Type       GUID
	Size       int64
	Attributes uint64
}

// Disk describes a whole image. Seed should be unique to the image, for
// example its name and version; every GUID on the disk is derived from it.
type Disk struct {
	Size       int64
	Seed       string
	Partitions []Partition
}

// Placement is a partition with the position and GUID it was given.
type Placement struct {
	Partition
	GUID     GUID
	FirstLBA uint64
	LastLBA  uint64
}

// Offset is where the partition's first byte sits in the image.
func (p Placement) Offset() int64 { return int64(p.FirstLBA) * SectorSize }

// Layout is the result of planning a disk.
type Layout struct {
	DiskGUID   GUID
	Sectors    uint64
	Partitions []Placement
}

func alignUp(lba uint64) uint64 {
	return (lba + alignSectors - 1) / alignSectors * alignSectors
}

// Plan checks a disk description and works out where each partition goes,
// without writing anything.
func Plan(d Disk) (Layout, error) {
	if d.Seed == "" {
		return Layout{}, errors.New("gpt: the disk needs a seed so its GUIDs are reproducible; use something unique to the image, such as its name and version")
	}
	if d.Size <= 0 || d.Size%SectorSize != 0 {
		return Layout{}, fmt.Errorf("gpt: disk size %d is not a positive multiple of %d bytes", d.Size, SectorSize)
	}
	if len(d.Partitions) > entryCount {
		return Layout{}, fmt.Errorf("gpt: %d partitions requested, a GPT holds at most %d", len(d.Partitions), entryCount)
	}
	sectors := uint64(d.Size / SectorSize)
	layout := Layout{DiskGUID: deriveGUID(d.Seed, "disk"), Sectors: sectors}
	next := uint64(firstUsableLBA)
	for i, p := range d.Partitions {
		if p.Size <= 0 || p.Size%SectorSize != 0 {
			return Layout{}, fmt.Errorf("gpt: partition %d (%q) has size %d, which is not a positive multiple of %d bytes", i, p.Name, p.Size, SectorSize)
		}
		if p.Type == (GUID{}) {
			return Layout{}, fmt.Errorf("gpt: partition %d (%q) has no type GUID", i, p.Name)
		}
		if n := len(utf16.Encode([]rune(p.Name))); n > nameUnits {
			return Layout{}, fmt.Errorf("gpt: partition name %q is %d UTF-16 units long, the limit is %d", p.Name, n, nameUnits)
		}
		first := alignUp(next)
		last := first + uint64(p.Size/SectorSize) - 1
		layout.Partitions = append(layout.Partitions, Placement{
			Partition: p,
			GUID:      deriveGUID(d.Seed, fmt.Sprintf("partition %d", i)),
			FirstLBA:  first,
			LastLBA:   last,
		})
		next = last + 1
	}
	needed := (next + backupSectors) * SectorSize
	if uint64(d.Size) < needed {
		return Layout{}, fmt.Errorf("gpt: the partitions need a disk of at least %d bytes but it has %d; it is %d bytes (%d KiB) short",
			needed, d.Size, needed-uint64(d.Size), (needed-uint64(d.Size)+1023)/1024)
	}
	return layout, nil
}

// Write plans the disk and writes the protective MBR and both copies of the
// partition table into w. Partition contents are left to the caller, who
// writes them at each Placement's Offset.
func Write(w io.WriterAt, d Disk) (Layout, error) {
	layout, err := Plan(d)
	if err != nil {
		return Layout{}, err
	}
	entries := encodeEntries(layout.Partitions)
	entriesCRC := crc32.ChecksumIEEE(entries)
	last := layout.Sectors - 1
	primary := encodeHeader(1, last, firstUsableLBA, last-backupSectors, layout.DiskGUID, 2, entriesCRC)
	backup := encodeHeader(last, 1, firstUsableLBA, last-backupSectors, layout.DiskGUID, last-entrySectors, entriesCRC)

	writes := []struct {
		lba  uint64
		data []byte
	}{
		{0, encodeProtectiveMBR(layout.Sectors)},
		{1, primary},
		{2, entries},
		{last - entrySectors, entries},
		{last, backup},
	}
	for _, wr := range writes {
		if _, err := w.WriteAt(wr.data, int64(wr.lba)*SectorSize); err != nil {
			return Layout{}, fmt.Errorf("gpt: write at sector %d: %w", wr.lba, err)
		}
	}
	return layout, nil
}

// encodeProtectiveMBR follows section 5.2.3: one partition record of type
// 0xEE covering the whole disk, so tools that only know MBR leave it alone.
func encodeProtectiveMBR(sectors uint64) []byte {
	mbr := make([]byte, SectorSize)
	rec := mbr[446:462]
	rec[2] = 0x02
	rec[4] = 0xEE
	end := chs(sectors - 1)
	copy(rec[5:8], end[:])
	binary.LittleEndian.PutUint32(rec[8:], 1)
	binary.LittleEndian.PutUint32(rec[12:], uint32(min(sectors-1, 0xFFFFFFFF)))
	mbr[510], mbr[511] = 0x55, 0xAA
	return mbr
}

// chs converts a block address to the cylinder, head and sector triple old
// BIOSes used, assuming 255 heads and 63 sectors per track. Addresses past
// cylinder 1023 cannot be expressed and become FF FF FF, as the spec asks.
func chs(lba uint64) [3]byte {
	const heads, perTrack = 255, 63
	c := lba / (heads * perTrack)
	if c > 1023 {
		return [3]byte{0xFF, 0xFF, 0xFF}
	}
	h := lba / perTrack % heads
	s := lba%perTrack + 1
	return [3]byte{byte(h), byte(s) | byte(c>>8)<<6, byte(c)}
}

// encodeHeader lays out the 92-byte header from table 5.5. The CRC covers the
// header with its own CRC field set to zero.
func encodeHeader(my, alternate, firstUsable, lastUsable uint64, disk GUID, entriesLBA uint64, entriesCRC uint32) []byte {
	sector := make([]byte, SectorSize)
	h := sector[:headerSize]
	copy(h[0:8], "EFI PART")
	binary.LittleEndian.PutUint32(h[8:], 0x00010000)
	binary.LittleEndian.PutUint32(h[12:], headerSize)
	binary.LittleEndian.PutUint64(h[24:], my)
	binary.LittleEndian.PutUint64(h[32:], alternate)
	binary.LittleEndian.PutUint64(h[40:], firstUsable)
	binary.LittleEndian.PutUint64(h[48:], lastUsable)
	copy(h[56:72], disk[:])
	binary.LittleEndian.PutUint64(h[72:], entriesLBA)
	binary.LittleEndian.PutUint32(h[80:], entryCount)
	binary.LittleEndian.PutUint32(h[84:], entrySize)
	binary.LittleEndian.PutUint32(h[88:], entriesCRC)
	binary.LittleEndian.PutUint32(h[16:], crc32.ChecksumIEEE(h))
	return sector
}

// encodeEntries builds the full 128-entry array from table 5.6; unused
// entries stay zero.
func encodeEntries(parts []Placement) []byte {
	buf := make([]byte, entryCount*entrySize)
	for i, p := range parts {
		e := buf[i*entrySize : (i+1)*entrySize]
		copy(e[0:16], p.Type[:])
		copy(e[16:32], p.GUID[:])
		binary.LittleEndian.PutUint64(e[32:], p.FirstLBA)
		binary.LittleEndian.PutUint64(e[40:], p.LastLBA)
		binary.LittleEndian.PutUint64(e[48:], p.Attributes)
		for j, u := range utf16.Encode([]rune(p.Name)) {
			binary.LittleEndian.PutUint16(e[56+2*j:], u)
		}
	}
	return buf
}
