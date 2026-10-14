package gpt

import (
	"bytes"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"hash/crc32"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"unicode/utf16"
)

const mib = 1 << 20

// The reader below is written from the UEFI specification, tables 5.1, 5.5
// and 5.6, independently of the writer.

type mbrRecord struct {
	bootIndicator byte
	startCHS      [3]byte
	osType        byte
	endCHS        [3]byte
	startLBA      uint32
	sizeLBA       uint32
}

type header struct {
	signature   string
	revision    uint32
	size        uint32
	crc         uint32
	reserved    uint32
	myLBA       uint64
	altLBA      uint64
	firstUsable uint64
	lastUsable  uint64
	diskGUID    GUID
	entriesLBA  uint64
	numEntries  uint32
	entrySize   uint32
	entriesCRC  uint32
}

type entry struct {
	typ, unique GUID
	first, last uint64
	attributes  uint64
	name        string
}

func readMBRRecord(img []byte, n int) mbrRecord {
	r := img[446+16*n : 446+16*(n+1)]
	var rec mbrRecord
	rec.bootIndicator = r[0]
	copy(rec.startCHS[:], r[1:4])
	rec.osType = r[4]
	copy(rec.endCHS[:], r[5:8])
	rec.startLBA = binary.LittleEndian.Uint32(r[8:12])
	rec.sizeLBA = binary.LittleEndian.Uint32(r[12:16])
	return rec
}

func readHeader(img []byte, lba uint64) header {
	b := img[lba*512 : lba*512+92]
	var h header
	h.signature = string(b[0:8])
	h.revision = binary.LittleEndian.Uint32(b[8:12])
	h.size = binary.LittleEndian.Uint32(b[12:16])
	h.crc = binary.LittleEndian.Uint32(b[16:20])
	h.reserved = binary.LittleEndian.Uint32(b[20:24])
	h.myLBA = binary.LittleEndian.Uint64(b[24:32])
	h.altLBA = binary.LittleEndian.Uint64(b[32:40])
	h.firstUsable = binary.LittleEndian.Uint64(b[40:48])
	h.lastUsable = binary.LittleEndian.Uint64(b[48:56])
	copy(h.diskGUID[:], b[56:72])
	h.entriesLBA = binary.LittleEndian.Uint64(b[72:80])
	h.numEntries = binary.LittleEndian.Uint32(b[80:84])
	h.entrySize = binary.LittleEndian.Uint32(b[84:88])
	h.entriesCRC = binary.LittleEndian.Uint32(b[88:92])
	return h
}

func headerCRC(img []byte, lba uint64) uint32 {
	b := append([]byte(nil), img[lba*512:lba*512+92]...)
	clear(b[16:20])
	return crc32.ChecksumIEEE(b)
}

func readEntries(img []byte, lba uint64) []entry {
	var out []entry
	for i := range 128 {
		b := img[lba*512+uint64(i)*128 : lba*512+uint64(i+1)*128]
		var e entry
		copy(e.typ[:], b[0:16])
		copy(e.unique[:], b[16:32])
		e.first = binary.LittleEndian.Uint64(b[32:40])
		e.last = binary.LittleEndian.Uint64(b[40:48])
		e.attributes = binary.LittleEndian.Uint64(b[48:56])
		var units []uint16
		for j := 56; j < 128; j += 2 {
			u := binary.LittleEndian.Uint16(b[j:])
			if u == 0 {
				break
			}
			units = append(units, u)
		}
		e.name = string(utf16.Decode(units))
		out = append(out, e)
	}
	return out
}

func writeImage(t *testing.T, d Disk) ([]byte, Layout, string) {
	t.Helper()
	path := filepath.Join(t.TempDir(), "disk.img")
	f, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := f.Truncate(d.Size); err != nil {
		t.Fatal(err)
	}
	layout, err := Write(f, d)
	if err != nil {
		t.Fatalf("Write: %v", err)
	}
	if err := f.Close(); err != nil {
		t.Fatal(err)
	}
	img, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return img, layout, path
}

func espDisk() Disk {
	return Disk{
		Size: 64 * mib,
		Seed: "meridian test image",
		Partitions: []Partition{
			{Name: "EFI system", Type: EFISystemPartition, Size: 33 * mib},
		},
	}
}

func twoPartitionDisk() Disk {
	return Disk{
		Size: 8 * mib,
		Seed: "two partitions",
		Partitions: []Partition{
			{Name: "EFI system", Type: EFISystemPartition, Size: mib + 512},
			{Name: "Meridian", Type: MeridianSystem, Size: 2 * mib, Attributes: 1},
		},
	}
}

func TestProtectiveMBR(t *testing.T) {
	img, _, _ := writeImage(t, espDisk())
	if !bytes.Equal(img[:446], make([]byte, 446)) {
		t.Error("boot code area should be zero")
	}
	rec := readMBRRecord(img, 0)
	// 64 MiB is 131072 sectors; LBA 131071 is cylinder 8, head 40, sector 32.
	want := mbrRecord{
		bootIndicator: 0,
		startCHS:      [3]byte{0x00, 0x02, 0x00},
		osType:        0xEE,
		endCHS:        [3]byte{40, 32, 8},
		startLBA:      1,
		sizeLBA:       131071,
	}
	if rec != want {
		t.Errorf("protective record = %+v\nwant %+v", rec, want)
	}
	for n := 1; n < 4; n++ {
		if r := readMBRRecord(img, n); r != (mbrRecord{}) {
			t.Errorf("record %d should be empty, got %+v", n, r)
		}
	}
	if img[510] != 0x55 || img[511] != 0xAA {
		t.Errorf("boot signature = % x, want 55 aa", img[510:512])
	}
}

func TestPrimaryHeader(t *testing.T) {
	img, layout, _ := writeImage(t, espDisk())
	h := readHeader(img, 1)
	sectors := uint64(64 * mib / 512)
	want := header{
		signature:   "EFI PART",
		revision:    0x00010000,
		size:        92,
		crc:         headerCRC(img, 1),
		myLBA:       1,
		altLBA:      sectors - 1,
		firstUsable: 34,
		lastUsable:  sectors - 34,
		diskGUID:    layout.DiskGUID,
		entriesLBA:  2,
		numEntries:  128,
		entrySize:   128,
		entriesCRC:  crc32.ChecksumIEEE(img[2*512 : 34*512]),
	}
	if h != want {
		t.Errorf("primary header = %+v\nwant %+v", h, want)
	}
	if h.crc == 0 {
		t.Error("header CRC is zero, it was never filled in")
	}
	if !bytes.Equal(img[512+92:1024], make([]byte, 512-92)) {
		t.Error("the rest of the header sector should be zero")
	}
}

func TestBackupHeaderMirrorsThePrimary(t *testing.T) {
	img, layout, _ := writeImage(t, twoPartitionDisk())
	last := layout.Sectors - 1
	b := readHeader(img, last)
	p := readHeader(img, 1)
	if b.signature != "EFI PART" || b.myLBA != last || b.altLBA != 1 || b.entriesLBA != last-32 {
		t.Errorf("backup header positions wrong: %+v", b)
	}
	if b.crc != headerCRC(img, last) {
		t.Errorf("backup header CRC %08x does not match its contents (%08x)", b.crc, headerCRC(img, last))
	}
	if b.diskGUID != p.diskGUID || b.entriesCRC != p.entriesCRC || b.firstUsable != p.firstUsable || b.lastUsable != p.lastUsable {
		t.Errorf("backup header disagrees with primary:\nbackup  %+v\nprimary %+v", b, p)
	}
	primaryEntries := img[2*512 : 34*512]
	backupEntries := img[(last-32)*512 : last*512]
	if !bytes.Equal(primaryEntries, backupEntries) {
		t.Error("backup entry array differs from the primary one")
	}
	if crc32.ChecksumIEEE(backupEntries) != b.entriesCRC {
		t.Error("backup entry array does not match the CRC in the backup header")
	}
}

func TestEntriesAreAlignedAndComplete(t *testing.T) {
	d := twoPartitionDisk()
	img, layout, _ := writeImage(t, d)
	entries := readEntries(img, 2)

	esp, mer := entries[0], entries[1]
	if esp.typ != EFISystemPartition || esp.first != 2048 || esp.last != 2048+2049-1 || esp.attributes != 0 || esp.name != "EFI system" {
		t.Errorf("first entry = %+v", esp)
	}
	// The first partition ends at LBA 4096, so the second starts at the next
	// 1 MiB boundary, LBA 6144.
	if mer.typ != MeridianSystem || mer.first != 6144 || mer.last != 6144+4096-1 || mer.attributes != 1 || mer.name != "Meridian" {
		t.Errorf("second entry = %+v", mer)
	}
	if esp.unique != layout.Partitions[0].GUID || mer.unique != layout.Partitions[1].GUID {
		t.Error("unique GUIDs on disk differ from the returned layout")
	}
	if esp.unique == mer.unique || esp.unique == layout.DiskGUID {
		t.Error("partition GUIDs must be distinct from each other and from the disk GUID")
	}
	for i, e := range entries[2:] {
		if e != (entry{}) {
			t.Errorf("entry %d should be unused, got %+v", i+2, e)
		}
	}
	got := layout.Partitions
	if got[0].FirstLBA != 2048 || got[0].LastLBA != 4096 || got[1].FirstLBA != 6144 || got[1].LastLBA != 10239 {
		t.Errorf("layout = %+v", got)
	}
	if got[1].Offset() != 6144*512 {
		t.Errorf("Offset = %d, want %d", got[1].Offset(), 6144*512)
	}
}

func TestNamesAreUTF16(t *testing.T) {
	d := espDisk()
	d.Partitions[0].Name = "Système ✓ 🚀"
	img, _, _ := writeImage(t, d)
	if got := readEntries(img, 2)[0].name; got != "Système ✓ 🚀" {
		t.Errorf("name = %q", got)
	}
}

func TestNameLimitCountsUTF16Units(t *testing.T) {
	d := espDisk()
	d.Partitions[0].Name = strings.Repeat("a", 34) + "🚀"
	if _, err := Plan(d); err != nil {
		t.Errorf("36 units should fit: %v", err)
	}
	d.Partitions[0].Name = strings.Repeat("a", 35) + "🚀"
	_, err := Plan(d)
	if err == nil || !strings.Contains(err.Error(), "37 UTF-16 units long, the limit is 36") {
		t.Errorf("37 units: err = %v", err)
	}
}

func TestPartitionContentsAreLeftAlone(t *testing.T) {
	d := twoPartitionDisk()
	path := filepath.Join(t.TempDir(), "disk.img")
	pattern := bytes.Repeat([]byte{0xA5}, int(d.Size))
	if err := os.WriteFile(path, pattern, 0o644); err != nil {
		t.Fatal(err)
	}
	f, err := os.OpenFile(path, os.O_RDWR, 0)
	if err != nil {
		t.Fatal(err)
	}
	layout, err := Write(f, d)
	f.Close()
	if err != nil {
		t.Fatal(err)
	}
	img, _ := os.ReadFile(path)
	for _, p := range layout.Partitions {
		region := img[p.FirstLBA*512 : (p.LastLBA+1)*512]
		if !bytes.Equal(region, pattern[:len(region)]) {
			t.Errorf("partition %q was overwritten", p.Name)
		}
	}
}

func TestSameInputGivesTheSameImage(t *testing.T) {
	a, _, _ := writeImage(t, twoPartitionDisk())
	b, _, _ := writeImage(t, twoPartitionDisk())
	if !bytes.Equal(a, b) {
		t.Fatal("two writes of the same disk differ")
	}
	d := twoPartitionDisk()
	d.Seed = "another image"
	c, _, _ := writeImage(t, d)
	if readHeader(a, 1).diskGUID == readHeader(c, 1).diskGUID {
		t.Error("a different seed should give a different disk GUID")
	}
	if readEntries(a, 2)[0].unique == readEntries(c, 2)[0].unique {
		t.Error("a different seed should give different partition GUIDs")
	}
}

func TestDerivedGUIDsAreVersion4(t *testing.T) {
	for _, label := range []string{"disk", "partition 0", "partition 1", ""} {
		g := deriveGUID("seed", label)
		c := g.canonical()
		if c[6]>>4 != 4 {
			t.Errorf("%q: version nibble = %x, want 4 (%s)", label, c[6]>>4, g)
		}
		if c[8]>>6 != 2 {
			t.Errorf("%q: variant bits = %b, want 10 (%s)", label, c[8]>>6, g)
		}
	}
	if deriveGUID("a", "x") == deriveGUID("a", "y") || deriveGUID("a", "x") == deriveGUID("b", "x") {
		t.Error("seed and label must both change the GUID")
	}
	if deriveGUID("ab", "c") == deriveGUID("a", "bc") {
		t.Error("seed and label must be kept apart before hashing")
	}
}

func TestGUIDByteOrder(t *testing.T) {
	want := GUID{0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B}
	if EFISystemPartition != want {
		t.Errorf("ESP type on disk = % X\nwant                % X", EFISystemPartition[:], want[:])
	}
	if s := EFISystemPartition.String(); s != "C12A7328-F81F-11D2-BA4B-00A0C93EC93B" {
		t.Errorf("String = %s", s)
	}
	if s := MeridianSystem.String(); s != "B0DD29AB-2286-43EF-9809-E345D01A1B29" {
		t.Errorf("Meridian type = %s", s)
	}
	lower, err := ParseGUID("c12a7328-f81f-11d2-ba4b-00a0c93ec93b")
	if err != nil || lower != EFISystemPartition {
		t.Errorf("lower case parse = %v, %v", lower, err)
	}
}

func TestParseGUIDRejectsMalformedText(t *testing.T) {
	for _, s := range []string{
		"",
		"C12A7328F81F11D2BA4B00A0C93EC93B",
		"C12A732-F81F-11D2-BA4B-00A0C93EC93B",
		"C12A73288-F81F-11D2-BA4B-00A0C93EC93B",
		"C12A7328-F81-11D2-BA4B-00A0C93EC93B",
		"C12A7328-F81F-11D-BA4B-00A0C93EC93B",
		"C12A7328-F81F-11D2-BA4-00A0C93EC93B",
		"C12A7328-F81F-11D2-BA4B-00A0C93EC93",
		"C12A7328-F81F-11D2-BA4B-00A0C93EC93B-00",
	} {
		if _, err := ParseGUID(s); err == nil || !strings.Contains(err.Error(), "expected the form") {
			t.Errorf("ParseGUID(%q) err = %v", s, err)
		}
	}
	if _, err := ParseGUID("G12A7328-F81F-11D2-BA4B-00A0C93EC93B"); err == nil || !strings.Contains(err.Error(), "not hexadecimal") {
		t.Errorf("non-hex err = %v", err)
	}
}

func TestMustParseGUIDPanicsOnBadInput(t *testing.T) {
	defer func() {
		if recover() == nil {
			t.Error("mustParseGUID accepted a malformed GUID")
		}
	}()
	mustParseGUID("not a guid")
}

func TestPlanRefusesBadDisks(t *testing.T) {
	esp := Partition{Name: "EFI", Type: EFISystemPartition, Size: mib}
	tests := []struct {
		name string
		disk Disk
		want string
	}{
		{"no seed", Disk{Size: 8 * mib}, "needs a seed"},
		{"zero size", Disk{Seed: "s"}, "disk size 0 is not a positive multiple of 512"},
		{"negative size", Disk{Seed: "s", Size: -512}, "disk size -512 is not"},
		{"odd size", Disk{Seed: "s", Size: 8*mib + 1}, "is not a positive multiple"},
		{"too many partitions", Disk{Seed: "s", Size: 8 * mib, Partitions: make([]Partition, 129)}, "129 partitions requested, a GPT holds at most 128"},
		{"empty partition", Disk{Seed: "s", Size: 8 * mib, Partitions: []Partition{{Name: "x", Type: EFISystemPartition}}}, `partition 0 ("x") has size 0`},
		{"negative partition", Disk{Seed: "s", Size: 8 * mib, Partitions: []Partition{{Name: "x", Type: EFISystemPartition, Size: -512}}}, "has size -512"},
		{"ragged partition", Disk{Seed: "s", Size: 8 * mib, Partitions: []Partition{{Name: "x", Type: EFISystemPartition, Size: 1000}}}, "has size 1000"},
		{"no type", Disk{Seed: "s", Size: 8 * mib, Partitions: []Partition{esp, {Name: "y", Size: mib}}}, `partition 1 ("y") has no type GUID`},
		{"too big", Disk{Seed: "s", Size: 2 * mib, Partitions: []Partition{{Name: "x", Type: EFISystemPartition, Size: 2 * mib}}},
			"need a disk of at least 3162624 bytes but it has 2097152; it is 1065472 bytes (1041 KiB) short"},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			_, err := Plan(tt.disk)
			if err == nil || !strings.Contains(err.Error(), tt.want) {
				t.Errorf("err = %v, want it to contain %q", err, tt.want)
			}
		})
	}
}

func TestPartitionThatExactlyFillsTheDisk(t *testing.T) {
	// 2048 sectors before the partition, 33 after it for the backup table.
	exact := int64(2048+4096+33) * 512
	d := Disk{Seed: "s", Size: exact, Partitions: []Partition{{Name: "x", Type: EFISystemPartition, Size: 2 * mib}}}
	layout, err := Plan(d)
	if err != nil {
		t.Fatalf("exact fit refused: %v", err)
	}
	if lastUsable := layout.Sectors - 34; layout.Partitions[0].LastLBA != lastUsable {
		t.Errorf("partition ends at %d, last usable LBA is %d", layout.Partitions[0].LastLBA, lastUsable)
	}
	d.Size -= 512
	if _, err := Plan(d); err == nil || !strings.Contains(err.Error(), "it is 512 bytes (1 KiB) short") {
		t.Errorf("one sector short: err = %v", err)
	}
}

func TestEmptyDiskNeedsOnlyTheTables(t *testing.T) {
	if _, err := Plan(Disk{Seed: "s", Size: 67 * 512}); err != nil {
		t.Errorf("67 sectors hold both tables: %v", err)
	}
	if _, err := Plan(Disk{Seed: "s", Size: 66 * 512}); err == nil {
		t.Error("66 sectors cannot hold both tables")
	}
}

func TestCHS(t *testing.T) {
	tests := []struct {
		lba  uint64
		want [3]byte
	}{
		{0, [3]byte{0, 1, 0}},
		{1, [3]byte{0, 2, 0}},
		{62, [3]byte{0, 63, 0}},
		{63, [3]byte{1, 1, 0}},
		{16064, [3]byte{254, 63, 0}},
		{16065, [3]byte{0, 1, 1}},
		{256 * 16065, [3]byte{0, 0x41, 0}},
		{1023*16065 + 16064, [3]byte{254, 0xFF, 0xFF}},
		{1024 * 16065, [3]byte{0xFF, 0xFF, 0xFF}},
	}
	for _, tt := range tests {
		if got := chs(tt.lba); got != tt.want {
			t.Errorf("chs(%d) = % x, want % x", tt.lba, got, tt.want)
		}
	}
}

func TestMBRSizeIsCappedOnHugeDisks(t *testing.T) {
	tests := []struct {
		sectors uint64
		want    uint32
	}{
		{0xFFFFFFFF, 0xFFFFFFFE},
		{0x100000000, 0xFFFFFFFF},
		{1 << 40, 0xFFFFFFFF},
	}
	for _, tt := range tests {
		mbr := encodeProtectiveMBR(tt.sectors)
		if got := binary.LittleEndian.Uint32(mbr[446+12:]); got != tt.want {
			t.Errorf("%d sectors: size field = %#x, want %#x", tt.sectors, got, tt.want)
		}
		if !bytes.Equal(mbr[446+5:446+8], []byte{0xFF, 0xFF, 0xFF}) {
			t.Errorf("%d sectors: end CHS = % x, want ff ff ff", tt.sectors, mbr[446+5:446+8])
		}
	}
}

func TestWriteReportsWriterErrors(t *testing.T) {
	path := filepath.Join(t.TempDir(), "readonly.img")
	if err := os.WriteFile(path, make([]byte, 64*mib), 0o644); err != nil {
		t.Fatal(err)
	}
	f, err := os.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	_, err = Write(f, espDisk())
	if err == nil || !strings.Contains(err.Error(), "gpt: write at sector 0") {
		t.Fatalf("err = %v", err)
	}
	var pathErr *os.PathError
	if !errors.As(err, &pathErr) {
		t.Errorf("the file system error should be wrapped, got %T", errors.Unwrap(err))
	}
}

func TestWriteRefusesWhatPlanRefuses(t *testing.T) {
	f, err := os.Create(filepath.Join(t.TempDir(), "x.img"))
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	if _, err := Write(f, Disk{Size: mib}); err == nil {
		t.Fatal("Write accepted a disk without a seed")
	}
	if info, _ := f.Stat(); info.Size() != 0 {
		t.Errorf("a refused disk still wrote %d bytes", info.Size())
	}
}

func requireTool(t *testing.T, name string) string {
	t.Helper()
	path, err := exec.LookPath(name)
	if err != nil {
		if os.Getenv("SEXTANT_REQUIRE_TOOLS") == "1" {
			t.Fatalf("%s is not installed and SEXTANT_REQUIRE_TOOLS=1", name)
		}
		t.Skipf("%s is not installed, skipping the cross-check", name)
	}
	return path
}

// requireUtilLinux is requireTool for the util-linux tools, since macOS
// has an fdisk of its own that reads and prints differently.
func requireUtilLinux(t *testing.T, name string) string {
	t.Helper()
	path := requireTool(t, name)
	out, _ := exec.Command(path, "--version").CombinedOutput()
	if !strings.Contains(string(out), "util-linux") {
		if os.Getenv("SEXTANT_REQUIRE_TOOLS") == "1" {
			t.Fatalf("%s is not the one from util-linux and SEXTANT_REQUIRE_TOOLS=1: %s", name, out)
		}
		t.Skipf("%s is not the one from util-linux, skipping the cross-check", name)
	}
	return path
}

// sfdisk from util-linux reads the image on its own; if it sees the same
// table we meant to write, the encoding is right.
func TestSfdiskReadsTheTable(t *testing.T) {
	sfdisk := requireUtilLinux(t, "sfdisk")
	_, layout, path := writeImage(t, twoPartitionDisk())
	out, err := exec.Command(sfdisk, "--json", path).Output()
	if err != nil {
		t.Fatalf("sfdisk --json: %v", err)
	}
	var doc struct {
		Table struct {
			Label      string `json:"label"`
			ID         string `json:"id"`
			FirstLBA   uint64 `json:"firstlba"`
			LastLBA    uint64 `json:"lastlba"`
			SectorSize int    `json:"sectorsize"`
			Partitions []struct {
				Start uint64 `json:"start"`
				Size  uint64 `json:"size"`
				Type  string `json:"type"`
				UUID  string `json:"uuid"`
				Name  string `json:"name"`
				Attrs string `json:"attrs"`
			} `json:"partitions"`
		} `json:"partitiontable"`
	}
	if err := json.Unmarshal(out, &doc); err != nil {
		t.Fatalf("parse sfdisk output: %v\n%s", err, out)
	}
	tb := doc.Table
	if tb.Label != "gpt" || tb.ID != layout.DiskGUID.String() || tb.FirstLBA != 34 || tb.LastLBA != layout.Sectors-34 || tb.SectorSize != 512 {
		t.Errorf("sfdisk sees %+v", tb)
	}
	if len(tb.Partitions) != 2 {
		t.Fatalf("sfdisk sees %d partitions, want 2:\n%s", len(tb.Partitions), out)
	}
	for i, p := range tb.Partitions {
		want := layout.Partitions[i]
		if p.Start != want.FirstLBA || p.Size != want.LastLBA-want.FirstLBA+1 || p.Type != want.Type.String() || p.UUID != want.GUID.String() || p.Name != want.Name {
			t.Errorf("partition %d: sfdisk sees %+v, wrote %+v", i, p, want)
		}
	}
	if tb.Partitions[1].Attrs != "RequiredPartition" {
		t.Errorf("attribute bit 0 should read as RequiredPartition, sfdisk says %q", tb.Partitions[1].Attrs)
	}

	verify, err := exec.Command(sfdisk, "--verify", path).CombinedOutput()
	if err != nil || !strings.Contains(string(verify), "No errors detected") {
		t.Errorf("sfdisk --verify: %v\n%s", err, verify)
	}
}

func TestFdiskListsTheDisk(t *testing.T) {
	fdisk := requireUtilLinux(t, "fdisk")
	_, layout, path := writeImage(t, espDisk())
	out, err := exec.Command(fdisk, "-l", path).CombinedOutput()
	if err != nil {
		t.Fatalf("fdisk -l: %v\n%s", err, out)
	}
	text := string(out)
	for _, want := range []string{
		"Disklabel type: gpt",
		"Disk identifier: " + layout.DiskGUID.String(),
		fmt.Sprintf("%s1     2048 69631   67584  33M EFI System", path),
	} {
		if !strings.Contains(strings.Join(strings.Fields(text), " "), strings.Join(strings.Fields(want), " ")) {
			t.Errorf("fdisk output lacks %q:\n%s", want, text)
		}
	}
	if strings.Contains(strings.ToLower(text), "corrupt") || strings.Contains(strings.ToLower(text), "mismatch") {
		t.Errorf("fdisk complains:\n%s", text)
	}
}

// An image built today must be rebuilt byte for byte by later versions of
// sextant, so the way GUIDs come out of the seed may never change. The values
// were computed separately with Python's hashlib.
func TestGUIDDerivationNeverChanges(t *testing.T) {
	layout, err := Plan(twoPartitionDisk())
	if err != nil {
		t.Fatal(err)
	}
	want := []string{
		"A0590ACD-9EF4-4681-8680-B42B5F991E99",
		"18BB7F67-4AAA-4E46-8706-C06FB1ADF2F7",
		"3DB9531A-A1A4-4ECA-80DE-69EFD0F7CFE9",
	}
	got := []string{layout.DiskGUID.String(), layout.Partitions[0].GUID.String(), layout.Partitions[1].GUID.String()}
	for i := range want {
		if got[i] != want[i] {
			t.Errorf("GUID %d = %s, want %s", i, got[i], want[i])
		}
	}
}

func TestPartitionEndingOnABoundaryIsFollowedDirectly(t *testing.T) {
	d := Disk{Seed: "s", Size: 8 * mib, Partitions: []Partition{
		{Name: "a", Type: EFISystemPartition, Size: mib},
		{Name: "b", Type: MeridianSystem, Size: mib},
	}}
	layout, err := Plan(d)
	if err != nil {
		t.Fatal(err)
	}
	if a, b := layout.Partitions[0], layout.Partitions[1]; a.LastLBA != 4095 || b.FirstLBA != 4096 {
		t.Errorf("a ends at %d, b starts at %d; want 4095 and 4096", a.LastLBA, b.FirstLBA)
	}
}

func TestAFullTableOf128Partitions(t *testing.T) {
	parts := make([]Partition, 128)
	for i := range parts {
		parts[i] = Partition{Name: fmt.Sprintf("p%d", i), Type: MeridianSystem, Size: 512}
	}
	layout, err := Plan(Disk{Seed: "s", Size: 130 * mib, Partitions: parts})
	if err != nil {
		t.Fatalf("128 partitions should fit in the table: %v", err)
	}
	if last := layout.Partitions[127]; last.FirstLBA != 2048*128 || last.LastLBA != 2048*128 {
		t.Errorf("partition 127 at %d..%d", last.FirstLBA, last.LastLBA)
	}
}
