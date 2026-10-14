package iso

import (
	"bytes"
	"encoding/binary"
	"fmt"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/disk/fat"
)

// The reader below follows ECMA-119 and the El Torito specification on its
// own; it never calls into the writer.

type reader struct {
	t   *testing.T
	img []byte
}

func (r *reader) block(lba uint32) []byte {
	r.t.Helper()
	off := int(lba) * 2048
	if off+2048 > len(r.img) {
		r.t.Fatalf("block %d is outside the %d-byte image", lba, len(r.img))
	}
	return r.img[off : off+2048]
}

// both32 reads a both-byte-order field (section 7.3.3) and fails when the
// two halves disagree.
func (r *reader) both32(b []byte) uint32 {
	r.t.Helper()
	le, be := binary.LittleEndian.Uint32(b[0:]), binary.BigEndian.Uint32(b[4:])
	if le != be {
		r.t.Fatalf("both-endian field disagrees: %d and %d", le, be)
	}
	return le
}

func (r *reader) both16(b []byte) uint16 {
	r.t.Helper()
	le, be := binary.LittleEndian.Uint16(b[0:]), binary.BigEndian.Uint16(b[2:])
	if le != be {
		r.t.Fatalf("both-endian field disagrees: %d and %d", le, be)
	}
	return le
}

type record struct {
	length  int
	extAttr byte
	lba     uint32
	size    uint32
	date    [7]byte
	flags   byte
	unit    byte
	gap     byte
	seq     uint16
	id      string
}

func (r *reader) parseRecord(b []byte) record {
	r.t.Helper()
	rec := record{
		length:  int(b[0]),
		extAttr: b[1],
		lba:     r.both32(b[2:10]),
		size:    r.both32(b[10:18]),
		flags:   b[25],
		unit:    b[26],
		gap:     b[27],
		seq:     r.both16(b[28:32]),
	}
	copy(rec.date[:], b[18:25])
	idLen := int(b[32])
	rec.id = string(b[33 : 33+idLen])
	if want := 33 + idLen + (idLen+1)%2; rec.length != want {
		r.t.Fatalf("record %q has length %d, want %d", rec.id, rec.length, want)
	}
	if idLen%2 == 0 && b[33+idLen] != 0 {
		r.t.Fatalf("record %q has a non-zero padding byte", rec.id)
	}
	return rec
}

// readDir walks a directory extent block by block. A zero length byte ends
// the records in a block (section 6.8.1.1), so a record that would cross into
// the next block shows up as a broken one.
func (r *reader) readDir(lba, size uint32) []record {
	r.t.Helper()
	if size%2048 != 0 {
		r.t.Fatalf("directory at %d has size %d, not whole blocks", lba, size)
	}
	var out []record
	for b := range size / 2048 {
		blk := r.block(lba + b)
		for off := 0; off < 2048 && blk[off] != 0; {
			if off+int(blk[off]) > 2048 {
				r.t.Fatalf("record at block %d offset %d crosses into the next block", lba+b, off)
			}
			rec := r.parseRecord(blk[off:])
			out = append(out, rec)
			off += rec.length
		}
	}
	return out
}

type walked struct {
	files map[string][]byte
	dirs  map[string]record
	order map[string][]string
}

func displayName(id string) string {
	id, _, _ = strings.Cut(id, ";")
	return strings.TrimSuffix(id, ".")
}

func (r *reader) walk() walked {
	r.t.Helper()
	pvd := r.block(16)
	root := r.parseRecord(pvd[156:190])
	w := walked{files: map[string][]byte{}, dirs: map[string]record{"": root}, order: map[string][]string{}}
	var walk func(prefix string, dir, parent record)
	walk = func(prefix string, dir, parent record) {
		recs := r.readDir(dir.lba, dir.size)
		if len(recs) < 2 || recs[0].id != "\x00" || recs[1].id != "\x01" {
			r.t.Fatalf("directory %q does not start with its . and .. records", prefix)
		}
		if recs[0].lba != dir.lba || recs[0].size != dir.size || recs[0].flags != 2 {
			r.t.Errorf("%q: . record %+v does not describe the directory itself", prefix, recs[0])
		}
		if recs[1].lba != parent.lba || recs[1].size != parent.size || recs[1].flags != 2 {
			r.t.Errorf("%q: .. record %+v does not describe the parent", prefix, recs[1])
		}
		for _, rec := range recs[2:] {
			w.order[prefix] = append(w.order[prefix], rec.id)
			path := prefix + displayName(rec.id)
			if rec.flags&2 != 0 {
				w.dirs[path] = rec
				walk(path+"/", rec, dir)
				continue
			}
			if !strings.HasSuffix(rec.id, ";1") {
				r.t.Errorf("file %q has no version number", rec.id)
			}
			end := int64(rec.lba)*2048 + int64(rec.size)
			if end > int64(len(r.img)) {
				r.t.Fatalf("file %q runs past the end of the image", path)
			}
			w.files[path] = r.img[int64(rec.lba)*2048 : end]
		}
	}
	walk("", root, root)
	return w
}

type ptEntry struct {
	id     string
	lba    uint32
	parent uint16
}

func (r *reader) pathTable(lba uint32, size int, order binary.ByteOrder) []ptEntry {
	r.t.Helper()
	b := r.img[int(lba)*2048 : int(lba)*2048+size]
	var out []ptEntry
	for off := 0; off < len(b); {
		n := int(b[off])
		if b[off+1] != 0 {
			r.t.Fatalf("path table record at %d has an extended attribute length", off)
		}
		out = append(out, ptEntry{
			id:     string(b[off+8 : off+8+n]),
			lba:    order.Uint32(b[off+2:]),
			parent: order.Uint16(b[off+6:]),
		})
		off += 8 + n + n%2
	}
	return out
}

var fixedTime = time.Date(2026, 10, 14, 13, 37, 59, 270_000_000, time.UTC)

func pattern(n int, seed byte) []byte {
	b := make([]byte, n)
	for i := range b {
		b[i] = byte(i*13) ^ seed
	}
	return b
}

func sampleImage() Image {
	return Image{
		VolumeID: "MERIDIAN",
		Time:     fixedTime,
		Files: map[string][]byte{
			"EFI/BOOT/BOOTX64.EFI": pattern(3000, 1),
			"meridian/boot.meri":   []byte("timeout = 3s\n"),
			"meridian/kernel.elf":  pattern(5000, 2),
			"readme":               nil,
			"Sub/Deeper/x.y":       pattern(2048, 3),
		},
		BootImage: pattern(8192, 4),
	}
}

func writeImage(t *testing.T, img Image) ([]byte, int64, string) {
	t.Helper()
	path := filepath.Join(t.TempDir(), "meridian.iso")
	f, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	size, err := Write(f, img)
	if cerr := f.Close(); cerr != nil {
		t.Fatal(cerr)
	}
	if err != nil {
		t.Fatalf("Write: %v", err)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if int64(len(data)) != size {
		t.Fatalf("Write reported %d bytes, the file has %d", size, len(data))
	}
	return data, size, path
}

func spaces(n int) string { return strings.Repeat(" ", n) }

func TestVolumeDescriptors(t *testing.T) {
	img, size, _ := writeImage(t, sampleImage())
	r := &reader{t: t, img: img}
	if !bytes.Equal(img[:16*2048], make([]byte, 16*2048)) {
		t.Error("the system area should be zero")
	}
	for lba, typ := range map[uint32]byte{16: 1, 17: 0, 18: 255} {
		b := r.block(lba)
		if b[0] != typ || string(b[1:6]) != "CD001" || b[6] != 1 {
			t.Errorf("descriptor at %d: type %d, id %q, version %d", lba, b[0], b[1:6], b[6])
		}
	}
	if !bytes.Equal(r.block(18)[7:], make([]byte, 2041)) {
		t.Error("the terminator should be zero after its header")
	}

	p := r.block(16)
	ptSize := r.both32(p[132:140])
	checks := []struct {
		name string
		got  any
		want any
	}{
		{"system id", string(p[8:40]), spaces(32)},
		{"volume id", string(p[40:72]), "MERIDIAN" + spaces(24)},
		{"volume space", r.both32(p[80:88]), uint32(size / 2048)},
		{"set size", r.both16(p[120:124]), uint16(1)},
		{"sequence", r.both16(p[124:128]), uint16(1)},
		{"block size", r.both16(p[128:132]), uint16(2048)},
		{"L table", binary.LittleEndian.Uint32(p[140:]), uint32(20)},
		{"optional L table", binary.LittleEndian.Uint32(p[144:]), uint32(0)},
		{"M table", binary.BigEndian.Uint32(p[148:]), uint32(21)},
		{"optional M table", binary.BigEndian.Uint32(p[152:]), uint32(0)},
		{"volume set", string(p[190:318]), spaces(128)},
		{"publisher", string(p[318:446]), spaces(128)},
		{"preparer", string(p[446:574]), spaces(128)},
		{"application", string(p[574:702]), "SEXTANT" + spaces(121)},
		{"file ids", string(p[702:813]), spaces(111)},
		{"created", string(p[813:830]), "2026101413375927\x00"},
		{"modified", string(p[830:847]), "2026101413375927\x00"},
		{"expires", string(p[847:864]), "0000000000000000\x00"},
		{"effective", string(p[864:881]), "0000000000000000\x00"},
		{"structure version", p[881], byte(1)},
	}
	for _, c := range checks {
		if c.got != c.want {
			t.Errorf("%s = %q, want %q", c.name, c.got, c.want)
		}
	}
	for _, zero := range [][2]int{{7, 8}, {72, 80}, {88, 120}, {882, 2048}} {
		if !bytes.Equal(p[zero[0]:zero[1]], make([]byte, zero[1]-zero[0])) {
			t.Errorf("bytes %d to %d of the PVD should be zero", zero[0], zero[1])
		}
	}
	root := r.parseRecord(p[156:190])
	if root.length != 34 || root.id != "\x00" || root.flags != 2 || root.seq != 1 || root.size != 2048 {
		t.Errorf("root record = %+v", root)
	}
	// Each record is 8 bytes and the identifier, padded to an even length:
	// root 10, EFI 12, MERIDIAN 16, SUB 12, BOOT 12, DEEPER 14.
	if ptSize != 10+12+16+12+12+14 {
		t.Errorf("path table size = %d", ptSize)
	}
}

func TestFilesRoundTrip(t *testing.T) {
	in := sampleImage()
	img, _, _ := writeImage(t, in)
	w := (&reader{t: t, img: img}).walk()
	want := map[string][]byte{}
	for p, data := range in.Files {
		want[strings.ToUpper(p)] = data
	}
	if len(w.files) != len(want) {
		t.Errorf("found %d files, wrote %d", len(w.files), len(want))
	}
	for p, data := range want {
		got, ok := w.files[p]
		if !ok {
			t.Errorf("%q is missing; found %v", p, keysOf(w.files))
			continue
		}
		if !bytes.Equal(got, data) {
			t.Errorf("%q differs: %d bytes, want %d", p, len(got), len(data))
		}
	}
	for _, d := range []string{"EFI", "EFI/BOOT", "MERIDIAN", "SUB", "SUB/DEEPER"} {
		if _, ok := w.dirs[d]; !ok {
			t.Errorf("directory %q is missing", d)
		}
	}
	if rec := w.dirs["MERIDIAN"]; rec.size != 2048 {
		t.Errorf("MERIDIAN directory size = %d", rec.size)
	}
}

func keysOf[V any](m map[string]V) []string {
	var out []string
	for k := range m {
		out = append(out, k)
	}
	slices.Sort(out)
	return out
}

func TestIdentifiersOnDisk(t *testing.T) {
	img, _, _ := writeImage(t, sampleImage())
	w := (&reader{t: t, img: img}).walk()
	want := map[string][]string{
		"":          {"EFI", "MERIDIAN", "README.;1", "SUB"},
		"MERIDIAN/": {"BOOT.MERI;1", "KERNEL.ELF;1"},
		"SUB/":      {"DEEPER"},
		"EFI/BOOT/": {"BOOTX64.EFI;1"},
	}
	for dir, ids := range want {
		if !slices.Equal(w.order[dir], ids) {
			t.Errorf("%q holds %q, want %q", dir, w.order[dir], ids)
		}
	}
}

func TestFilesAreContiguousAndDoNotOverlap(t *testing.T) {
	img, _, _ := writeImage(t, sampleImage())
	r := &reader{t: t, img: img}
	w := r.walk()
	type extent struct {
		name       string
		start, end uint32
	}
	var exts []extent
	for name, rec := range w.dirs {
		exts = append(exts, extent{"dir " + name, rec.lba, rec.lba + rec.size/2048})
	}
	for _, d := range w.dirs {
		for _, rec := range r.readDir(d.lba, d.size)[2:] {
			if rec.flags&2 == 0 && rec.size > 0 {
				exts = append(exts, extent{rec.id, rec.lba, rec.lba + (rec.size+2047)/2048})
			}
		}
	}
	slices.SortFunc(exts, func(a, b extent) int { return int(a.start) - int(b.start) })
	for i := 1; i < len(exts); i++ {
		if exts[i].start < exts[i-1].end {
			t.Errorf("%s overlaps %s", exts[i].name, exts[i-1].name)
		}
	}
	if exts[0].start < 22 {
		t.Errorf("%s starts at block %d, inside the descriptors or path tables", exts[0].name, exts[0].start)
	}
}

func TestEmptyFilesHaveNoExtent(t *testing.T) {
	img, _, _ := writeImage(t, sampleImage())
	r := &reader{t: t, img: img}
	for _, rec := range r.readDir(r.walk().dirs[""].lba, 2048) {
		if rec.id == "README.;1" && (rec.lba != 0 || rec.size != 0) {
			t.Errorf("empty file record = %+v", rec)
		}
	}
}

func TestDirectorySpanningBlocks(t *testing.T) {
	in := sampleImage()
	in.Files = map[string][]byte{}
	for i := range 100 {
		in.Files[fmt.Sprintf("many/FILE_NUMBER_%03d.TXT", i)] = []byte{byte(i)}
	}
	img, _, _ := writeImage(t, in)
	w := (&reader{t: t, img: img}).walk()
	// Each record is 33 + 21 bytes: 37 fit in a block, so 102 records
	// including . and .. need three blocks.
	if size := w.dirs["MANY"].size; size != 3*2048 {
		t.Errorf("directory size = %d, want three blocks", size)
	}
	if len(w.files) != 100 {
		t.Errorf("found %d of 100 files", len(w.files))
	}
	for i := range 100 {
		p := fmt.Sprintf("MANY/FILE_NUMBER_%03d.TXT", i)
		if !bytes.Equal(w.files[p], []byte{byte(i)}) {
			t.Errorf("%q is wrong", p)
		}
	}
	if !slices.IsSortedFunc(w.order["MANY/"], compareIDs) {
		t.Error("records are not in identifier order")
	}
}

func TestCompareIDs(t *testing.T) {
	tests := []struct {
		a, b string
		want int
	}{
		{"A", "B", -1},
		{"A", "AB", -1},
		{"AB", "A_B", -1},
		{"A.B;1", "A.AB;1", 1},
		{"A.B;1", "A.B;1", 0},
		{"A.;1", "A.B;1", -1},
		{"Z;1", "AA;1", 1},
		{"README.;1", "README.TXT;1", -1},
		{"EFI", "EFI_X", -1},
		{"A.B;1", "A.B1;1", -1},
		{"AB", "A.Z;1", 1},
	}
	for _, tt := range tests {
		if got := compareIDs(tt.a, tt.b); got != tt.want {
			t.Errorf("compareIDs(%q, %q) = %d, want %d", tt.a, tt.b, got, tt.want)
		}
		if got := compareIDs(tt.b, tt.a); got != -tt.want {
			t.Errorf("compareIDs(%q, %q) = %d, want %d", tt.b, tt.a, got, -tt.want)
		}
	}
}

func TestPathTables(t *testing.T) {
	img, _, _ := writeImage(t, sampleImage())
	r := &reader{t: t, img: img}
	p := r.block(16)
	size := int(r.both32(p[132:140]))
	l := r.pathTable(binary.LittleEndian.Uint32(p[140:]), size, binary.LittleEndian)
	m := r.pathTable(binary.BigEndian.Uint32(p[148:]), size, binary.BigEndian)
	if !slices.Equal(l, m) {
		t.Fatalf("L and M path tables differ:\n%v\n%v", l, m)
	}
	w := r.walk()
	want := []struct {
		path   string
		parent uint16
	}{
		{"", 1}, {"EFI", 1}, {"MERIDIAN", 1}, {"SUB", 1}, {"EFI/BOOT", 2}, {"SUB/DEEPER", 4},
	}
	if len(l) != len(want) {
		t.Fatalf("path table has %d entries: %v", len(l), l)
	}
	for i, e := range l {
		name := want[i].path[strings.LastIndex(want[i].path, "/")+1:]
		if want[i].path == "" {
			name = "\x00"
		}
		if e.id != name || e.parent != want[i].parent || e.lba != w.dirs[want[i].path].lba {
			t.Errorf("entry %d = %+v, want %q under %d at block %d", i+1, e, name, want[i].parent, w.dirs[want[i].path].lba)
		}
	}
	if raw := img[20*2048 : 20*2048+size]; raw[1] != 0 || raw[9] != 0 {
		t.Error("the root entry should be followed by its padding byte")
	}
}

func TestRecordDates(t *testing.T) {
	in := sampleImage()
	in.Time = time.Date(2026, 10, 14, 15, 37, 59, 0, time.FixedZone("CEST", 2*3600))
	img, _, _ := writeImage(t, in)
	r := &reader{t: t, img: img}
	want := [7]byte{126, 10, 14, 13, 37, 59, 0}
	w := r.walk()
	for name, rec := range w.dirs {
		if rec.date != want {
			t.Errorf("%q date = %v, want %v (UTC)", name, rec.date, want)
		}
	}
	if got := string(r.block(16)[813:830]); got != "2026101413375900\x00" {
		t.Errorf("volume creation date = %q", got)
	}
}

func TestBootRecordAndCatalog(t *testing.T) {
	in := sampleImage()
	img, size, _ := writeImage(t, in)
	r := &reader{t: t, img: img}
	br := r.block(17)
	if string(br[7:39]) != "EL TORITO SPECIFICATION"+strings.Repeat("\x00", 9) {
		t.Errorf("boot system id = %q", br[7:39])
	}
	if !bytes.Equal(br[39:71], make([]byte, 32)) || !bytes.Equal(br[75:], make([]byte, 2048-75)) {
		t.Error("unused boot record bytes should be zero")
	}
	catLBA := binary.LittleEndian.Uint32(br[71:])
	if catLBA != 19 {
		t.Errorf("catalog at block %d", catLBA)
	}
	cat := r.block(catLBA)
	v := cat[0:32]
	var sum uint16
	for i := 0; i < 32; i += 2 {
		sum += binary.LittleEndian.Uint16(v[i:])
	}
	if v[0] != 1 || v[1] != 0xEF || v[2] != 0 || v[3] != 0 || v[30] != 0x55 || v[31] != 0xAA || sum != 0 {
		t.Errorf("validation entry % x, word sum %04x", v, sum)
	}
	if string(v[4:28]) != "MERIDIAN"+strings.Repeat("\x00", 16) {
		t.Errorf("id string = %q", v[4:28])
	}
	d := cat[32:64]
	rba := binary.LittleEndian.Uint32(d[8:])
	if d[0] != 0x88 || d[1] != 0 || binary.LittleEndian.Uint16(d[2:]) != 0 || d[4] != 0 || d[5] != 0 {
		t.Errorf("default entry header % x", d[:6])
	}
	if count := binary.LittleEndian.Uint16(d[6:]); count != 16 {
		t.Errorf("sector count = %d, want 16 for 8192 bytes", count)
	}
	if !bytes.Equal(d[12:], make([]byte, 20)) || !bytes.Equal(cat[64:], make([]byte, 2048-64)) {
		t.Error("the rest of the catalog should be zero")
	}
	if got := img[int(rba)*2048 : int(rba)*2048+len(in.BootImage)]; !bytes.Equal(got, in.BootImage) {
		t.Error("the boot image is not at the catalog's load RBA")
	}
	if int64(rba)*2048+int64(len(in.BootImage)) != size {
		t.Errorf("the boot image ends at %d, the image at %d; it must come last", int64(rba)*2048+int64(len(in.BootImage)), size)
	}
}

func TestSectorCountBoundary(t *testing.T) {
	for _, tt := range []struct {
		size int
		want uint16
	}{
		{512, 1},
		{0xFFFF * 512, 0xFFFF},
		{0x10000 * 512, 0},
		{64 << 20, 0},
	} {
		cat := bootCatalog(1234, tt.size)
		if got := binary.LittleEndian.Uint16(cat[32+6:]); got != tt.want {
			t.Errorf("%d bytes: sector count %d, want %d", tt.size, got, tt.want)
		}
		if lba := binary.LittleEndian.Uint32(cat[32+8:]); lba != 1234 {
			t.Errorf("load RBA = %d", lba)
		}
	}
}

func TestSameInputGivesTheSameImage(t *testing.T) {
	a, _, _ := writeImage(t, sampleImage())
	b, _, _ := writeImage(t, sampleImage())
	if !bytes.Equal(a, b) {
		t.Fatal("two writes of the same image differ")
	}
	in := sampleImage()
	in.Time = in.Time.Add(time.Second)
	c, _, _ := writeImage(t, in)
	if bytes.Equal(a, c) {
		t.Error("a different time should change the image")
	}
}

func TestRefusesBadImages(t *testing.T) {
	files := func(paths ...string) map[string][]byte {
		m := map[string][]byte{}
		for _, p := range paths {
			m[p] = []byte{1}
		}
		return m
	}
	tests := []struct {
		name   string
		change func(*Image)
		want   string
	}{
		{"long volume id", func(i *Image) { i.VolumeID = strings.Repeat("A", 33) }, "longer than 32 characters"},
		{"lower case volume id", func(i *Image) { i.VolumeID = "meridian" }, `contains 'm'`},
		{"dash in volume id", func(i *Image) { i.VolumeID = "A-B" }, `contains '-'`},
		{"too early", func(i *Image) { i.Time = time.Date(1899, 12, 31, 0, 0, 0, 0, time.UTC) }, "between 1900 and 2155"},
		{"too late", func(i *Image) { i.Time = time.Date(2156, 1, 1, 0, 0, 0, 0, time.UTC) }, "between 1900 and 2155"},
		{"no boot image", func(i *Image) { i.BootImage = nil }, "no boot image"},
		{"ragged boot image", func(i *Image) { i.BootImage = make([]byte, 1000) }, "1000 bytes, not a whole number of 512-byte sectors"},
		{"dash in name", func(i *Image) { i.Files = files("a-b") }, `"a-b" contains '-'`},
		{"period in directory", func(i *Image) { i.Files = files("dir.x/f") }, `directory name "dir.x" contains a period`},
		{"long directory", func(i *Image) { i.Files = files(strings.Repeat("D", 32) + "/f") }, "longer than 31 characters"},
		{"long file", func(i *Image) { i.Files = files(strings.Repeat("F", 28) + ".TXT") }, "longer than 30 characters"},
		{"two periods", func(i *Image) { i.Files = files("a.b.c") }, "more than one period"},
		{"empty name", func(i *Image) { i.Files = files("a//b") }, `name "" is empty`},
		{"leading slash", func(i *Image) { i.Files = files("/a") }, `name "" is empty`},
		{"too deep", func(i *Image) { i.Files = files("1/2/3/4/5/6/7/8/9") }, `"1/2/3/4/5/6/7/8/9" is 9 levels deep, ISO 9660 allows 8`},
		{"path too long", func(i *Image) { i.Files = files(longPath(25)) }, "256 bytes long as an ISO 9660 path, the limit is 255"},
		{"file and directory", func(i *Image) { i.Files = files("a", "a/b") }, `"a" is both a file and a directory`},
		{"case, directory first", func(i *Image) { i.Files = files("X/a", "x") }, `"X" and "x" differ only in case`},
		{"case, files", func(i *Image) { i.Files = files("README", "readme") }, `"README" and "readme" differ only in case`},
		{"case, nested", func(i *Image) { i.Files = files("p/Q/a", "p/q/b") }, `"p/Q" and "p/q" differ only in case`},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			in := sampleImage()
			tt.change(&in)
			path := filepath.Join(t.TempDir(), "x.iso")
			f, _ := os.Create(path)
			_, err := Write(f, in)
			f.Close()
			if err == nil || !strings.Contains(err.Error(), tt.want) {
				t.Errorf("err = %v, want %q", err, tt.want)
			}
			if info, _ := os.Stat(path); info.Size() != 0 {
				t.Errorf("a refused image still wrote %d bytes", info.Size())
			}
		})
	}
}

func TestLimitsThatStillFit(t *testing.T) {
	in := sampleImage()
	d := strings.Repeat("D", 31)
	in.VolumeID = strings.Repeat("V", 32)
	in.Files = map[string][]byte{
		strings.Repeat("F", 27) + ".TXT": {1},
		d + "/f":                         {2},
		"1/2/3/4/5/6/7/8":                {3},
		".hidden":                        {4},
	}
	if _, err := Write(newFile(t), in); err != nil {
		t.Errorf("limits: %v", err)
	}
	for _, tm := range []time.Time{time.Date(1900, 1, 1, 0, 0, 0, 0, time.UTC), time.Date(2155, 12, 31, 23, 59, 59, 0, time.UTC)} {
		in := sampleImage()
		in.Time = tm
		if _, err := Write(newFile(t), in); err != nil {
			t.Errorf("%s: %v", tm, err)
		}
	}
	in = sampleImage()
	in.Files = map[string][]byte{longPath(24): {1}}
	if _, err := Write(newFile(t), in); err != nil {
		t.Errorf("a path of exactly 255 bytes should fit: %v", err)
	}
}

// longPath builds seven directories of 31 characters and a file whose
// identifier is base + ".TXTX;1": 7*31 + 7 separators + base + 7 bytes.
func longPath(base int) string {
	d := strings.Repeat("D", 31)
	return strings.Join([]string{d, d, d, d, d, d, d, strings.Repeat("F", base) + ".TXTX"}, "/")
}

func newFile(t *testing.T) *os.File {
	t.Helper()
	f, err := os.Create(filepath.Join(t.TempDir(), "x.iso"))
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { f.Close() })
	return f
}

func TestWriterErrorsArePassedOn(t *testing.T) {
	path := filepath.Join(t.TempDir(), "ro.iso")
	os.WriteFile(path, nil, 0o644)
	f, _ := os.Open(path)
	defer f.Close()
	_, err := Write(f, sampleImage())
	if err == nil || !strings.Contains(err.Error(), "iso: write at block 0") {
		t.Fatalf("err = %v", err)
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

// A real EFI system partition from the fat package, as sextant will embed it.
func espImage(t *testing.T) []byte {
	t.Helper()
	path := filepath.Join(t.TempDir(), "esp.img")
	f, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	size := int64(64 << 20)
	if err := f.Truncate(size); err != nil {
		t.Fatal(err)
	}
	_, err = fat.Write(f, 0, fat.Volume{Size: size, Label: "MERIDIAN", Time: fixedTime, Files: map[string][]byte{
		"EFI/BOOT/BOOTX64.EFI": pattern(4000, 9),
	}})
	f.Close()
	if err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return data
}

// xorriso reads the boot catalog and the tree with its own code.
func TestXorrisoReadsTheImage(t *testing.T) {
	xorriso := requireTool(t, "xorriso")
	in := sampleImage()
	in.BootImage = espImage(t)
	img, _, path := writeImage(t, in)
	r := &reader{t: t, img: img}
	rba := binary.LittleEndian.Uint32(r.block(19)[32+8:])

	out, err := exec.Command(xorriso, "-indev", path, "-report_el_torito", "plain").CombinedOutput()
	if err != nil {
		t.Fatalf("xorriso: %v\n%s", err, out)
	}
	m := regexp.MustCompile(`El Torito boot img :\s+1\s+(\S+)\s+(\S+)\s+(\S+)\s+(\S+)\s+(\S+)\s+(\d+)\s+(\d+)`).FindStringSubmatch(string(out))
	if m == nil {
		t.Fatalf("no boot image line:\n%s", out)
	}
	if m[1] != "UEFI" || m[2] != "y" || m[3] != "none" || m[4] != "0x0000" || m[6] != "0" || m[7] != strconv.Itoa(int(rba)) {
		t.Errorf("xorriso sees platform %s bootable %s emulation %s segment %s size %s at %s; want UEFI y none 0x0000 0 at %d", m[1], m[2], m[3], m[4], m[6], m[7], rba)
	}
	if !strings.Contains(string(out), "Volume id    : 'MERIDIAN'") {
		t.Errorf("volume id missing:\n%s", out)
	}

	dir := t.TempDir()
	// xorriso gives extracted directories the read-only permissions of the
	// CD, which would stop the test from cleaning up after itself.
	t.Cleanup(func() {
		filepath.WalkDir(dir, func(p string, _ fs.DirEntry, _ error) error { return os.Chmod(p, 0o755) })
	})
	if out, err := exec.Command(xorriso, "-osirrox", "on", "-indev", path, "-extract", "/", dir).CombinedOutput(); err != nil {
		t.Fatalf("xorriso -extract: %v\n%s", err, out)
	}
	for p, data := range in.Files {
		got, err := os.ReadFile(filepath.Join(dir, strings.ToUpper(p)))
		if err != nil || !bytes.Equal(got, data) {
			t.Errorf("xorriso extracts %q as %d bytes (%v), want %d", p, len(got), err, len(data))
		}
	}
}

// bsdtar from libarchive is a second, unrelated ISO 9660 reader.
func TestBsdtarReadsTheTree(t *testing.T) {
	bsdtar := requireTool(t, "bsdtar")
	in := sampleImage()
	_, _, path := writeImage(t, in)
	dir := t.TempDir()
	if out, err := exec.Command(bsdtar, "-xf", path, "-C", dir).CombinedOutput(); err != nil {
		t.Fatalf("bsdtar: %v\n%s", err, out)
	}
	for p, data := range in.Files {
		got, err := os.ReadFile(filepath.Join(dir, strings.ToUpper(p)))
		if err != nil || !bytes.Equal(got, data) {
			t.Errorf("bsdtar extracts %q as %d bytes (%v), want %d", p, len(got), err, len(data))
		}
	}
}

// The embedded partition must still be a clean FAT32 volume once it is
// inside the ISO, at the place the catalog points to.
func TestEmbeddedPartitionIsIntact(t *testing.T) {
	fsck := requireTool(t, "fsck.fat")
	in := sampleImage()
	in.BootImage = espImage(t)
	img, _, _ := writeImage(t, in)
	r := &reader{t: t, img: img}
	cat := r.block(19)
	if count := binary.LittleEndian.Uint16(cat[32+6:]); count != 0 {
		t.Errorf("a 64 MiB image does not fit the 16-bit count; got %d, want 0", count)
	}
	rba := int(binary.LittleEndian.Uint32(cat[32+8:]))
	part := img[rba*2048:]
	if !bytes.Equal(part, in.BootImage) {
		t.Fatal("the bytes after the load RBA are not exactly the boot image")
	}
	path := filepath.Join(t.TempDir(), "extracted.img")
	if err := os.WriteFile(path, part, 0o644); err != nil {
		t.Fatal(err)
	}
	if out, err := exec.Command(fsck, "-n", path).CombinedOutput(); err != nil {
		t.Errorf("fsck.fat on the extracted partition: %v\n%s", err, out)
	}
}

// The image goes into whatever file the caller hands over. Every byte up to
// the reported size must be written, so old contents never leak in.
func TestDirtyTargetGivesTheSameImage(t *testing.T) {
	clean, size, _ := writeImage(t, sampleImage())
	path := filepath.Join(t.TempDir(), "dirty.iso")
	if err := os.WriteFile(path, bytes.Repeat([]byte{0xEE}, int(size)+4096), 0o644); err != nil {
		t.Fatal(err)
	}
	f, err := os.OpenFile(path, os.O_RDWR, 0)
	if err != nil {
		t.Fatal(err)
	}
	_, err = Write(f, sampleImage())
	f.Close()
	if err != nil {
		t.Fatal(err)
	}
	dirty, _ := os.ReadFile(path)
	if !bytes.Equal(dirty[:size], clean) {
		for i := range clean {
			if dirty[i] != clean[i] {
				t.Fatalf("byte %d (block %d) was not written: %02x", i, i/2048, dirty[i])
			}
		}
	}
}

// Problems are found in sorted path order, so running sextant twice on the
// same broken input gives the same message. Map order in Go is random, so a
// single run could pass by luck; twenty cannot.
func TestErrorsAreReportedInPathOrder(t *testing.T) {
	for range 20 {
		in := sampleImage()
		in.Files = map[string][]byte{"z-last": {1}, "b-second": {2}, "a-first": {3}, "m-middle": {4}}
		_, err := Write(newFile(t), in)
		if err == nil || !strings.Contains(err.Error(), `"a-first"`) {
			t.Fatalf("err = %v, want the first path in order", err)
		}
	}
}

func TestSubdirectoriesAreSortedToo(t *testing.T) {
	in := sampleImage()
	// Plain string order puts "a_b" before "ab"; upper case puts "AB" first,
	// because "B" sorts before "_".
	in.Files = map[string][]byte{"d/a_b": {1}, "d/ab": {2}, "d/e/a_b": {3}, "d/e/ab": {4}}
	img, _, _ := writeImage(t, in)
	w := (&reader{t: t, img: img}).walk()
	for _, dir := range []string{"D/", "D/E/"} {
		got := w.order[dir]
		if len(got) < 2 || got[0] != "AB.;1" || got[1] != "A_B.;1" {
			t.Errorf("%q holds %q, want AB.;1 before A_B.;1", dir, got)
		}
	}
}

// 68 bytes of "." and "..", then thirty records of 66 bytes, fill a block to
// the last byte. The thirtieth record still belongs in the first block.
func TestRecordThatEndsExactlyAtTheBlock(t *testing.T) {
	in := sampleImage()
	in.Files = map[string][]byte{}
	for i := range 30 {
		in.Files[fmt.Sprintf("full/%027d.TXT", i)] = []byte{byte(i)}
	}
	img, _, _ := writeImage(t, in)
	w := (&reader{t: t, img: img}).walk()
	if size := w.dirs["FULL"].size; size != 2048 {
		t.Errorf("directory size = %d, the records fit exactly in one block", size)
	}
	if len(w.files) != 30 {
		t.Errorf("found %d of 30 files", len(w.files))
	}
	in.Files["full/"+strings.Repeat("9", 27)+".TXT"] = []byte{1}
	img, _, _ = writeImage(t, in)
	if size := (&reader{t: t, img: img}).walk().dirs["FULL"].size; size != 4096 {
		t.Errorf("one more record should start a second block, size = %d", size)
	}
}

func TestEdgeCharactersAndSizesAreAccepted(t *testing.T) {
	in := sampleImage()
	in.VolumeID = "AZ_09"
	in.Files = map[string][]byte{"z0/a9_z.z0": {1}}
	in.BootImage = pattern(1536, 5)
	img, _, _ := writeImage(t, in)
	r := &reader{t: t, img: img}
	if got := binary.LittleEndian.Uint16(r.block(19)[32+6:]); got != 3 {
		t.Errorf("1536 bytes are 3 sectors, the catalog says %d", got)
	}
	if !bytes.Equal(r.walk().files["Z0/A9_Z.Z0"], []byte{1}) {
		t.Error("the file with edge characters is missing")
	}
}
