package fat

import (
	"bytes"
	"encoding/binary"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"testing"
	"time"
	"unicode/utf16"
)

const mib = 1 << 20

// The reader below follows fatgen103 on its own: it does not call into the
// writer, and it works out sizes and the FAT type from the boot sector the
// way any driver has to.

type bootRecord struct {
	jmp         [3]byte
	oem         string
	bytesPerSec uint16
	secPerClus  uint8
	reserved    uint16
	numFATs     uint8
	rootEntries uint16
	totSec16    uint16
	media       uint8
	fatSz16     uint16
	secPerTrk   uint16
	numHeads    uint16
	hidden      uint32
	totSec32    uint32
	fatSz32     uint32
	extFlags    uint16
	fsVer       uint16
	rootClus    uint32
	fsInfo      uint16
	bkBootSec   uint16
	drvNum      uint8
	bootSig     uint8
	volID       uint32
	volLab      string
	fsType      string
	signature   uint16
}

func parseBoot(b []byte) bootRecord {
	le16 := func(o int) uint16 { return binary.LittleEndian.Uint16(b[o:]) }
	le32 := func(o int) uint32 { return binary.LittleEndian.Uint32(b[o:]) }
	var r bootRecord
	copy(r.jmp[:], b[0:3])
	r.oem = string(b[3:11])
	r.bytesPerSec = le16(11)
	r.secPerClus = b[13]
	r.reserved = le16(14)
	r.numFATs = b[16]
	r.rootEntries = le16(17)
	r.totSec16 = le16(19)
	r.media = b[21]
	r.fatSz16 = le16(22)
	r.secPerTrk = le16(24)
	r.numHeads = le16(26)
	r.hidden = le32(28)
	r.totSec32 = le32(32)
	r.fatSz32 = le32(36)
	r.extFlags = le16(40)
	r.fsVer = le16(42)
	r.rootClus = le32(44)
	r.fsInfo = le16(48)
	r.bkBootSec = le16(50)
	r.drvNum = b[64]
	r.bootSig = b[66]
	r.volID = le32(67)
	r.volLab = string(b[71:82])
	r.fsType = string(b[82:90])
	r.signature = le16(510)
	return r
}

type volume struct {
	t            *testing.T
	img          []byte
	boot         bootRecord
	fats         [][]uint32
	dataStart    int64
	clusterBytes int64
	clusters     uint32
}

func openVolume(t *testing.T, img []byte) *volume {
	t.Helper()
	b := parseBoot(img)
	if b.bytesPerSec != 512 {
		t.Fatalf("bytes per sector = %d", b.bytesPerSec)
	}
	v := &volume{t: t, img: img, boot: b}
	fatBytes := int64(b.fatSz32) * 512
	for i := range int64(b.numFATs) {
		start := int64(b.reserved)*512 + i*fatBytes
		raw := img[start : start+fatBytes]
		entries := make([]uint32, len(raw)/4)
		for j := range entries {
			entries[j] = binary.LittleEndian.Uint32(raw[j*4:]) & 0x0FFFFFFF
		}
		v.fats = append(v.fats, entries)
	}
	v.dataStart = int64(b.reserved)*512 + int64(b.numFATs)*fatBytes
	v.clusterBytes = int64(b.secPerClus) * 512
	v.clusters = uint32((int64(b.totSec32)*512 - v.dataStart) / v.clusterBytes)
	// fatgen103 section 3.5: the cluster count alone decides the FAT type.
	if v.clusters < 65525 {
		t.Fatalf("only %d clusters, a driver would read this as FAT12 or FAT16", v.clusters)
	}
	return v
}

func (v *volume) chain(first uint32) []uint32 {
	v.t.Helper()
	var out []uint32
	seen := map[uint32]bool{}
	for c := first; ; {
		if c < 2 || c >= v.clusters+2 {
			v.t.Fatalf("chain from %d reaches cluster %d, outside the volume", first, c)
		}
		if seen[c] {
			v.t.Fatalf("chain from %d loops at cluster %d", first, c)
		}
		seen[c] = true
		out = append(out, c)
		next := v.fats[0][c]
		if next >= 0x0FFFFFF8 {
			return out
		}
		c = next
	}
}

func (v *volume) read(first uint32, size int64) []byte {
	var out []byte
	for _, c := range v.chain(first) {
		off := v.dataStart + int64(c-2)*v.clusterBytes
		out = append(out, v.img[off:off+v.clusterBytes]...)
	}
	return out[:size]
}

type dirEntry struct {
	name     string
	short    string
	hasLong  bool
	attr     byte
	first    uint32
	size     uint32
	crtTenth byte
	crtTime  uint16
	crtDate  uint16
	accDate  uint16
	wrtTime  uint16
	wrtDate  uint16
}

// specChecksum is the C routine from fatgen103 section 7, kept in its
// original shape so it differs from the writer's version.
func specChecksum(name []byte) byte {
	var sum byte
	for i := 11; i != 0; i-- {
		var hi byte
		if sum&1 != 0 {
			hi = 0x80
		}
		sum = hi + (sum >> 1) + name[11-i]
	}
	return sum
}

func (v *volume) readDir(first uint32) []dirEntry {
	v.t.Helper()
	raw := v.read(first, int64(len(v.chain(first)))*v.clusterBytes)
	var out []dirEntry
	var long []uint16
	expectSeq, sum := 0, byte(0)
	for off := 0; off < len(raw); off += 32 {
		e := raw[off : off+32]
		if e[0] == 0x00 {
			break
		}
		if e[11] == 0x0F {
			seq := int(e[0] & 0x1F)
			if e[0]&0x40 != 0 {
				if expectSeq != 0 {
					v.t.Fatalf("a new long name starts before the last one finished")
				}
				expectSeq, sum, long = seq, e[13], make([]uint16, seq*13)
			}
			if seq != expectSeq || e[13] != sum || e[12] != 0 || e[26] != 0 || e[27] != 0 {
				v.t.Fatalf("broken long name entry at %d: % x", off, e)
			}
			var units []uint16
			for _, o := range []int{1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30} {
				units = append(units, binary.LittleEndian.Uint16(e[o:]))
			}
			copy(long[(seq-1)*13:], units)
			expectSeq--
			continue
		}
		d := dirEntry{
			short:    string(e[0:11]),
			attr:     e[11],
			crtTenth: e[13],
			crtTime:  binary.LittleEndian.Uint16(e[14:]),
			crtDate:  binary.LittleEndian.Uint16(e[16:]),
			accDate:  binary.LittleEndian.Uint16(e[18:]),
			first:    uint32(binary.LittleEndian.Uint16(e[20:]))<<16 | uint32(binary.LittleEndian.Uint16(e[26:])),
			wrtTime:  binary.LittleEndian.Uint16(e[22:]),
			wrtDate:  binary.LittleEndian.Uint16(e[24:]),
			size:     binary.LittleEndian.Uint32(e[28:]),
		}
		if long != nil {
			if expectSeq != 0 {
				v.t.Fatalf("long name for %q is missing parts", d.short)
			}
			if specChecksum(e[0:11]) != sum {
				v.t.Fatalf("long name checksum %02x does not match short name %q (%02x)", sum, d.short, specChecksum(e[0:11]))
			}
			end := len(long)
			for i, u := range long {
				if u == 0 {
					end = i
					for _, pad := range long[i+1:] {
						if pad != 0xFFFF {
							v.t.Fatalf("long name of %q is not padded with FFFF after its terminator", d.short)
						}
					}
					break
				}
			}
			d.name, d.hasLong = string(utf16.Decode(long[:end])), true
			long = nil
		} else {
			base := strings.TrimRight(d.short[:8], " ")
			ext := strings.TrimRight(d.short[8:], " ")
			d.name = base
			if ext != "" {
				d.name += "." + ext
			}
		}
		out = append(out, d)
	}
	return out
}

type tree struct {
	files map[string][]byte
	dirs  map[string]uint32
	ents  map[string]dirEntry
}

func (v *volume) walk() tree {
	v.t.Helper()
	tr := tree{files: map[string][]byte{}, dirs: map[string]uint32{"": v.boot.rootClus}, ents: map[string]dirEntry{}}
	var walk func(prefix string, first, parent uint32)
	walk = func(prefix string, first, parent uint32) {
		for i, e := range v.readDir(first) {
			if e.attr == 0x08 {
				continue
			}
			if e.short == ".          " || e.short == "..         " {
				want := map[string]uint32{".          ": first, "..         ": parent}[e.short]
				if prefix == "" || e.attr != 0x10 || e.first != want || i > 1 {
					v.t.Fatalf("dot entry %q in %q: attr %02x cluster %d, want 10 and %d", e.short, prefix, e.attr, e.first, want)
				}
				continue
			}
			path := prefix + e.name
			tr.ents[path] = e
			if e.attr&0x10 != 0 {
				tr.dirs[path] = e.first
				up := first
				if prefix == "" {
					up = 0
				}
				walk(path+"/", e.first, up)
				continue
			}
			if e.size == 0 {
				if e.first != 0 {
					v.t.Fatalf("empty file %q points at cluster %d", path, e.first)
				}
				tr.files[path] = []byte{}
				continue
			}
			tr.files[path] = v.read(e.first, int64(e.size))
		}
	}
	walk("", v.boot.rootClus, 0)
	return tr
}

var fixedTime = time.Date(2026, 10, 14, 13, 37, 59, 270_000_000, time.UTC)

func pattern(n int, seed byte) []byte {
	b := make([]byte, n)
	for i := range b {
		b[i] = byte(i*7) ^ seed
	}
	return b
}

func sampleFiles() map[string][]byte {
	return map[string][]byte{
		"EFI/BOOT/BOOTX64.EFI":              pattern(1000, 1),
		"meridian/kernel.elf":               pattern(3*512+1, 2),
		"meridian/boot.meri":                []byte("timeout = 3s\n"),
		"empty.txt":                         {},
		"A Long File Name With Spaces.text": pattern(512, 3),
		"Système.cfg":                       []byte("ok"),
		"deep/a/b/c/d/file":                 pattern(10, 4),
		"thirteen-char":                     pattern(1, 5),
		"fourteen-chars":                    pattern(1, 6),
		"meridian/very/long/name/" + strings.Repeat("n", 40): pattern(2, 7),
	}
}

func sampleVolume() Volume {
	return Volume{Size: 64 * mib, Label: "Meridian", ID: 0x1234ABCD, HiddenSectors: 2048, Time: fixedTime, Files: sampleFiles()}
}

func writeVolume(t *testing.T, v Volume) ([]byte, Result, string) {
	t.Helper()
	return writeVolumeAt(t, v, 0)
}

func writeVolumeAt(t *testing.T, v Volume, offset int64) ([]byte, Result, string) {
	t.Helper()
	path := filepath.Join(t.TempDir(), "esp.img")
	f, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := f.Truncate(offset + v.Size); err != nil {
		t.Fatal(err)
	}
	res, err := Write(f, offset, v)
	if cerr := f.Close(); cerr != nil {
		t.Fatal(cerr)
	}
	if err != nil {
		t.Fatalf("Write: %v", err)
	}
	img, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return img, res, path
}

func TestBootSectorOf64MiBVolume(t *testing.T) {
	img, res, _ := writeVolume(t, sampleVolume())
	b := parseBoot(img)
	// fatgen103's formula: (131072 - 32) / ((256 * 1 + 2) / 2) rounded up.
	want := bootRecord{
		jmp: [3]byte{0xEB, 0x58, 0x90}, oem: "MSWIN4.1", bytesPerSec: 512, secPerClus: 1, reserved: 32, numFATs: 2,
		media: 0xF8, secPerTrk: 63, numHeads: 255, hidden: 2048, totSec32: 131072, fatSz32: 1016,
		rootClus: 2, fsInfo: 1, bkBootSec: 6, drvNum: 0x80, bootSig: 0x29, volID: 0x1234ABCD,
		volLab: "MERIDIAN   ", fsType: "FAT32   ", signature: 0xAA55,
	}
	if b != want {
		t.Errorf("boot sector = %+v\nwant         %+v", b, want)
	}
	if res != (Result{SectorsPerCluster: 1, SectorsPerFAT: 1016, Clusters: 131072 - 32 - 2*1016, UsedClusters: res.UsedClusters}) {
		t.Errorf("result = %+v", res)
	}
	if !bytes.Equal(img[90:510], make([]byte, 420)) {
		t.Error("boot code area should be zero")
	}
}

func TestBackupBootRecord(t *testing.T) {
	img, _, _ := writeVolume(t, sampleVolume())
	if !bytes.Equal(img[0:1024], img[6*512:8*512]) {
		t.Error("sectors 6 and 7 should copy the boot sector and FSInfo")
	}
	for _, s := range []int{2, 3, 4, 5, 8, 31} {
		if !bytes.Equal(img[s*512:(s+1)*512], make([]byte, 512)) {
			t.Errorf("reserved sector %d should be zero", s)
		}
	}
}

func TestFSInfo(t *testing.T) {
	img, res, _ := writeVolume(t, sampleVolume())
	fi := img[512:1024]
	le := func(o int) uint32 { return binary.LittleEndian.Uint32(fi[o:]) }
	if le(0) != 0x41615252 || le(484) != 0x61417272 || le(508) != 0xAA550000 {
		t.Errorf("FSInfo signatures: %08x %08x %08x", le(0), le(484), le(508))
	}
	if le(488) != res.Clusters-res.UsedClusters {
		t.Errorf("free count = %d, want %d", le(488), res.Clusters-res.UsedClusters)
	}
	if le(492) != 2+res.UsedClusters {
		t.Errorf("next free = %d, want %d", le(492), 2+res.UsedClusters)
	}
	if !bytes.Equal(fi[4:484], make([]byte, 480)) || !bytes.Equal(fi[496:508], make([]byte, 12)) {
		t.Error("reserved FSInfo bytes should be zero")
	}
}

func TestFilesRoundTrip(t *testing.T) {
	v := sampleVolume()
	img, _, _ := writeVolume(t, v)
	tr := openVolume(t, img).walk()
	if len(tr.files) != len(v.Files) {
		t.Errorf("found %d files, wrote %d", len(tr.files), len(v.Files))
	}
	for path, want := range v.Files {
		got, ok := tr.files[path]
		if !ok {
			t.Errorf("%q is missing", path)
			continue
		}
		if !bytes.Equal(got, want) {
			t.Errorf("%q has different contents (%d bytes, want %d)", path, len(got), len(want))
		}
		e := tr.ents[path]
		if e.attr != 0x20 || e.size != uint32(len(want)) {
			t.Errorf("%q: attr %02x size %d", path, e.attr, e.size)
		}
	}
	for _, dir := range []string{"EFI", "EFI/BOOT", "meridian", "deep/a/b/c/d", "meridian/very/long/name"} {
		if _, ok := tr.dirs[dir]; !ok {
			t.Errorf("directory %q is missing", dir)
		}
	}
}

func TestFilesAreContiguousAndChainsEnd(t *testing.T) {
	img, res, _ := writeVolume(t, sampleVolume())
	v := openVolume(t, img)
	tr := v.walk()
	used := map[uint32]string{}
	claim := func(name string, first uint32) {
		chain := v.chain(first)
		for i, c := range chain {
			if c != first+uint32(i) {
				t.Errorf("%q is fragmented: %v", name, chain)
			}
			if other, dup := used[c]; dup {
				t.Errorf("cluster %d belongs to %q and %q", c, other, name)
			}
			used[c] = name
		}
	}
	for path, first := range tr.dirs {
		claim("dir "+path, first)
	}
	for path, e := range tr.ents {
		if e.attr == 0x20 && e.size > 0 {
			claim(path, e.first)
			if n := len(v.chain(e.first)); n != (int(e.size)+511)/512 {
				t.Errorf("%q uses %d clusters for %d bytes", path, n, e.size)
			}
		}
	}
	if uint32(len(used)) != res.UsedClusters {
		t.Errorf("reachable clusters = %d, result says %d", len(used), res.UsedClusters)
	}
	for c := uint32(2); c < v.clusters+2; c++ {
		if _, ok := used[c]; !ok && v.fats[0][c] != 0 {
			t.Errorf("cluster %d is not used but its FAT entry is %08x", c, v.fats[0][c])
		}
	}
	if v.fats[0][0] != 0x0FFFFFF8 || v.fats[0][1] != 0x0FFFFFFF {
		t.Errorf("FAT[0] = %08x, FAT[1] = %08x", v.fats[0][0], v.fats[0][1])
	}
	if fmt.Sprint(v.fats[0]) != fmt.Sprint(v.fats[1]) {
		t.Error("the two FAT copies differ")
	}
}

func TestShortAndLongNames(t *testing.T) {
	img, _, _ := writeVolume(t, sampleVolume())
	tr := openVolume(t, img).walk()
	tests := []struct {
		path, short string
		long        bool
	}{
		{"EFI", "EFI        ", false},
		{"EFI/BOOT", "BOOT       ", false},
		{"EFI/BOOT/BOOTX64.EFI", "BOOTX64 EFI", false},
		{"meridian", "MERIDI~1   ", true},
		{"meridian/kernel.elf", "KERNEL~1ELF", true},
		{"meridian/boot.meri", "BOOT~1  MER", true},
		{"Système.cfg", "SYST_M~1CFG", true},
		{"A Long File Name With Spaces.text", "ALONGF~1TEX", true},
		{"empty.txt", "EMPTY~1 TXT", true},
	}
	for _, tt := range tests {
		e, ok := tr.ents[tt.path]
		if !ok {
			t.Errorf("%q not found", tt.path)
			continue
		}
		if e.short != tt.short || e.hasLong != tt.long {
			t.Errorf("%q: short %q long %v, want %q %v", tt.path, e.short, e.hasLong, tt.short, tt.long)
		}
	}
}

func TestNumericTailsAvoidCollisions(t *testing.T) {
	files := map[string][]byte{
		"KERNEL~1.ELF": {1},
		"kernel.elf":   {2},
		"kernelx.elf":  {3},
	}
	for c := 'a'; c <= 'j'; c++ {
		files["longname-"+string(c)+".txt"] = []byte{byte(c)}
	}
	img, _, _ := writeVolume(t, Volume{Size: 64 * mib, Time: fixedTime, Files: files})
	tr := openVolume(t, img).walk()
	want := map[string]string{
		"KERNEL~1.ELF":   "KERNEL~1ELF",
		"kernel.elf":     "KERNEL~2ELF",
		"kernelx.elf":    "KERNEL~3ELF",
		"longname-a.txt": "LONGNA~1TXT",
		"longname-i.txt": "LONGNA~9TXT",
		"longname-j.txt": "LONGN~10TXT",
	}
	for path, short := range want {
		if got := tr.ents[path].short; got != short {
			t.Errorf("%q: short name %q, want %q", path, got, short)
		}
	}
	for path, data := range files {
		if !bytes.Equal(tr.files[path], data) {
			t.Errorf("%q has the wrong contents", path)
		}
	}
}

func TestBasisName(t *testing.T) {
	tests := []struct{ in, base, ext string }{
		{"kernel.elf", "KERNEL", "ELF"},
		{"boot.meri", "BOOT", "MER"},
		{"my.long.file.name.txt", "MY", "TXT"},
		{".hidden", "HIDDEN", ""},
		{"..two.dots", "TWO", "DOT"},
		{"  spaced out .tx t", "SPACEDOU", "TXT"},
		{"Système.cfg", "SYST_ME", "CFG"},
		{"a+b,c;d=e[f]", "A_B_C_D_", ""},
		{"verylongbasename", "VERYLONG", ""},
		{"...", "_", ""},
		{" .hidden", "HIDDEN", ""},
		{"no-dot", "NO-DOT", ""},
		{"x.ÉÉÉÉ", "X", "___"},
	}
	for _, tt := range tests {
		base, ext := basisName(tt.in)
		if base != tt.base || ext != tt.ext {
			t.Errorf("basisName(%q) = %q, %q; want %q, %q", tt.in, base, ext, tt.base, tt.ext)
		}
	}
}

func TestPlainShortName(t *testing.T) {
	valid := map[string]string{
		"A":            "A          ",
		"ABCDEFGH.TXT": "ABCDEFGHTXT",
		"A.B":          "A       B  ",
		"$%'-_@~`":     "$%'-_@~`   ",
		"!(){}^#&.123": "!(){}^#&123",
	}
	for in, want := range valid {
		sn, ok := plainShortName(in)
		if !ok || string(sn[:]) != want {
			t.Errorf("plainShortName(%q) = %q, %v; want %q", in, sn, ok, want)
		}
	}
	for _, in := range []string{"a", "Abc", "ABCDEFGHI", "A.ABCD", "A.B.C", "A.", ".A", "A B", "É", "A+B", ""} {
		if _, ok := plainShortName(in); ok {
			t.Errorf("plainShortName(%q) accepted a name that needs a long entry", in)
		}
	}
}

func TestLongEntryLayout(t *testing.T) {
	sn := [11]byte([]byte("EXACTL~1   "))
	sum := specChecksum(sn[:])

	one := longEntries("exactly-thirt", sn)
	if len(one) != 32 {
		t.Fatalf("13 characters need one entry, got %d bytes", len(one))
	}
	if one[0] != 0x41 || one[11] != 0x0F || one[12] != 0 || one[13] != sum || one[26] != 0 || one[27] != 0 {
		t.Errorf("entry header = % x", one[:14])
	}
	if got := binary.LittleEndian.Uint16(one[30:]); got != 't' {
		t.Errorf("13th character = %q, a name that fills the entry has no terminator", rune(got))
	}

	two := longEntries("exactly-thirte", sn)
	if len(two) != 64 || two[0] != 0x42 || two[32] != 0x01 {
		t.Fatalf("14 characters: sequence bytes %02x %02x, length %d", two[0], two[32], len(two))
	}
	if binary.LittleEndian.Uint16(two[1:]) != 'e' || binary.LittleEndian.Uint16(two[3:]) != 0 {
		t.Errorf("second part should be 'e' then the terminator: % x", two[1:5])
	}
	for _, o := range []int{5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30} {
		if binary.LittleEndian.Uint16(two[o:]) != 0xFFFF {
			t.Errorf("padding at %d = %04x, want ffff", o, binary.LittleEndian.Uint16(two[o:]))
		}
	}
	if binary.LittleEndian.Uint16(two[32+1:]) != 'e' || binary.LittleEndian.Uint16(two[32+30:]) != 't' {
		t.Error("first part should hold characters 1 to 13")
	}
}

func TestChecksumMatchesTheSpecRoutine(t *testing.T) {
	for _, s := range []string{"KERNEL~1ELF", "README  TXT", "           ", "\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b", "ÿÿÿÿÿÿÿÿÿÿÿ"[:11]} {
		sn := [11]byte([]byte(s))
		if got, want := shortNameChecksum(sn), specChecksum(sn[:]); got != want {
			t.Errorf("checksum(%q) = %02x, spec routine says %02x", s, got, want)
		}
	}
}

func TestDirectorySpanningClusters(t *testing.T) {
	files := map[string][]byte{}
	for i := range 30 {
		files[fmt.Sprintf("many/a file with a long name %02d", i)] = []byte{byte(i)}
	}
	img, _, _ := writeVolume(t, Volume{Size: 64 * mib, Time: fixedTime, Files: files})
	v := openVolume(t, img)
	tr := v.walk()
	// 30 names of 26 characters take 3 entries each, plus "." and "..":
	// 92 entries of 32 bytes need 6 clusters of 512 bytes.
	if n := len(v.chain(tr.dirs["many"])); n != 6 {
		t.Errorf("directory uses %d clusters, want 6", n)
	}
	for path, data := range files {
		if !bytes.Equal(tr.files[path], data) {
			t.Errorf("%q lost", path)
		}
	}
}

func TestDotEntriesPointHomeAndUp(t *testing.T) {
	img, _, _ := writeVolume(t, sampleVolume())
	v := openVolume(t, img)
	tr := v.walk()
	check := func(dir string, parent uint32) {
		entries := v.readDir(tr.dirs[dir])
		if entries[0].short != ".          " || entries[0].first != tr.dirs[dir] || entries[0].attr != 0x10 {
			t.Errorf("%q: first entry %+v", dir, entries[0])
		}
		if entries[1].short != "..         " || entries[1].first != parent || entries[1].attr != 0x10 {
			t.Errorf("%q: second entry %+v, want parent cluster %d", dir, entries[1], parent)
		}
	}
	check("EFI", 0)
	check("EFI/BOOT", tr.dirs["EFI"])
	check("deep/a/b", tr.dirs["deep/a"])
}

func TestVolumeLabel(t *testing.T) {
	img, _, _ := writeVolume(t, sampleVolume())
	root := openVolume(t, img).readDir(2)
	if root[0].short != "MERIDIAN   " || root[0].attr != 0x08 || root[0].first != 0 || root[0].size != 0 {
		t.Errorf("first root entry = %+v, want the volume label", root[0])
	}

	v := sampleVolume()
	v.Label = ""
	img, _, _ = writeVolume(t, v)
	if parseBoot(img).volLab != "NO NAME    " {
		t.Errorf("label without a name = %q", parseBoot(img).volLab)
	}
	for _, e := range openVolume(t, img).readDir(2) {
		if e.attr == 0x08 {
			t.Errorf("no label was given, but the root holds %+v", e)
		}
	}
}

func TestLabelRules(t *testing.T) {
	if l, err := volumeLabel("abc 123"); err != nil || string(l[:]) != "ABC 123    " {
		t.Errorf("volumeLabel = %q, %v", l, err)
	}
	if l, err := volumeLabel("ELEVENCHARS"); err != nil || string(l[:]) != "ELEVENCHARS" {
		t.Errorf("11 characters: %q, %v", l, err)
	}
	if _, err := volumeLabel("TWELVECHARSX"); err == nil || !strings.Contains(err.Error(), "longer than 11") {
		t.Errorf("12 characters: %v", err)
	}
	if _, err := volumeLabel("A.B"); err == nil || !strings.Contains(err.Error(), `contains '.'`) {
		t.Errorf("period in label: %v", err)
	}
}

func TestTimestamps(t *testing.T) {
	img, _, _ := writeVolume(t, sampleVolume())
	tr := openVolume(t, img).walk()
	date := uint16(46<<9 | 10<<5 | 14)
	tm := uint16(13<<11 | 37<<5 | 29)
	for path, e := range tr.ents {
		if e.wrtDate != date || e.wrtTime != tm || e.crtDate != date || e.crtTime != tm || e.accDate != date || e.crtTenth != 127 {
			t.Errorf("%q: times %+v", path, e)
		}
	}
	st, _ := encodeTime(time.Date(1980, 1, 1, 0, 0, 0, 0, time.UTC))
	if st != (timestamp{date: 1<<5 | 1, time: 0, tenths: 0}) {
		t.Errorf("1980-01-01 = %+v", st)
	}
	end, err := encodeTime(time.Date(2107, 12, 31, 23, 59, 58, 990_000_000, time.UTC))
	if err != nil || end != (timestamp{date: 127<<9 | 12<<5 | 31, time: 23<<11 | 59<<5 | 29, tenths: 99}) {
		t.Errorf("2107-12-31 = %+v, %v", end, err)
	}
	for _, y := range []int{1979, 2108} {
		if _, err := encodeTime(time.Date(y, 6, 1, 0, 0, 0, 0, time.UTC)); err == nil || !strings.Contains(err.Error(), "between 1980 and 2107") {
			t.Errorf("year %d: %v", y, err)
		}
	}
}

func TestClusterSizeTable(t *testing.T) {
	tests := []struct {
		sectors uint32
		spc     uint32
	}{
		{66601, 1}, {532480, 1}, {532481, 8}, {16777216, 8}, {16777217, 16},
		{33554432, 16}, {33554433, 32}, {67108864, 32}, {67108865, 64}, {0xFFFFFFFF, 64},
	}
	for _, tt := range tests {
		g, err := planGeometry(int64(tt.sectors) * 512)
		if err != nil {
			t.Errorf("%d sectors: %v", tt.sectors, err)
			continue
		}
		if g.sectorsPerCluster != tt.spc {
			t.Errorf("%d sectors: %d sectors per cluster, want %d", tt.sectors, g.sectorsPerCluster, tt.spc)
		}
		if g.clusters < 65525 {
			t.Errorf("%d sectors: %d clusters is not FAT32", tt.sectors, g.clusters)
		}
		if uint64(g.fatSectors)*128 < uint64(g.clusters)+2 {
			t.Errorf("%d sectors: a FAT of %d sectors cannot index %d clusters", tt.sectors, g.fatSectors, g.clusters)
		}
		if data := uint64(g.totalSectors) - 32 - 2*uint64(g.fatSectors); uint64(g.clusters) != data/uint64(g.sectorsPerCluster) {
			t.Errorf("%d sectors: %d clusters, data region has room for %d", tt.sectors, g.clusters, data/uint64(g.sectorsPerCluster))
		}
	}
	g, _ := planGeometry(66601 * 512)
	if g.fatSectors != 517 || g.clusters != 65535 {
		t.Errorf("smallest volume: FAT %d sectors, %d clusters; want 517 and 65535", g.fatSectors, g.clusters)
	}
}

func TestGeometryRefusesBadSizes(t *testing.T) {
	tests := []struct {
		size int64
		want string
	}{
		{0, "volume size 0 is not a positive multiple of 512"},
		{-512, "volume size -512 is not"},
		{64*mib + 1, "is not a positive multiple"},
		{66600 * 512, "needs at least 34099712 bytes (66601 sectors), this one has 34099200"},
		{512 << 32, "above the FAT32 limit of 2 TiB"},
	}
	for _, tt := range tests {
		if _, err := planGeometry(tt.size); err == nil || !strings.Contains(err.Error(), tt.want) {
			t.Errorf("size %d: err = %v, want %q", tt.size, err, tt.want)
		}
	}
}

func TestVolumeThatIsExactlyFull(t *testing.T) {
	size := int64(67584 * 512)
	g, err := planGeometry(size)
	if err != nil {
		t.Fatal(err)
	}
	// The root directory takes one cluster, the file the rest.
	fill := int(g.clusters-1) * 512
	img, res, _ := writeVolume(t, Volume{Size: size, Time: fixedTime, Files: map[string][]byte{"FILL": make([]byte, fill)}})
	if res.UsedClusters != g.clusters {
		t.Fatalf("used %d of %d clusters", res.UsedClusters, g.clusters)
	}
	fi := img[512:1024]
	if free, next := binary.LittleEndian.Uint32(fi[488:]), binary.LittleEndian.Uint32(fi[492:]); free != 0 || next != 0xFFFFFFFF {
		t.Errorf("full volume: free %d, next %08x; want 0 and ffffffff", free, next)
	}

	f, _ := os.Create(filepath.Join(t.TempDir(), "x"))
	defer f.Close()
	_, err = Write(f, 0, Volume{Size: size, Time: fixedTime, Files: map[string][]byte{"FILL": make([]byte, fill+1)}})
	want := fmt.Sprintf("the files need %d clusters of 512 bytes but the volume has %d; it is 512 bytes (1 KiB) short", g.clusters+1, g.clusters)
	if err == nil || !strings.Contains(err.Error(), want) {
		t.Errorf("one byte too many: %v\nwant %q", err, want)
	}
}

func TestPathRules(t *testing.T) {
	tests := []struct {
		files map[string][]byte
		want  string
	}{
		{map[string][]byte{"": nil}, "empty path component"},
		{map[string][]byte{"/a": nil}, "empty path component"},
		{map[string][]byte{"a/": nil}, "empty path component"},
		{map[string][]byte{"a//b": nil}, "empty path component"},
		{map[string][]byte{"a/../b": nil}, `".." is not allowed`},
		{map[string][]byte{"./a": nil}, `"." is not allowed`},
		{map[string][]byte{"a*b": nil}, `contains '*'`},
		{map[string][]byte{"a:b": nil}, `contains ':'`},
		{map[string][]byte{"tab\there": nil}, `contains '\t'`},
		{map[string][]byte{"back\\slash": nil}, `contains '\\'`},
		{map[string][]byte{"dot.": nil}, "ends in a period or space"},
		{map[string][]byte{"space ": nil}, "ends in a period or space"},
		{map[string][]byte{strings.Repeat("x", 256): nil}, "longer than 255 characters"},
		{map[string][]byte{"a": {1}, "a/b": {2}}, `"a" is both a file and a directory`},
		{map[string][]byte{"Dir/x": {1}, "dir/y": {2}}, `"Dir" and "dir/y" differ only in case`},
		{map[string][]byte{"README": {1}, "readme": {2}}, `"README" and "readme" differ only in case`},
		{map[string][]byte{"x/Dir/a": {1}, "x/dir/b": {2}}, `"x/Dir" and "x/dir/b" differ only in case`},
		{map[string][]byte{"x/y/f": {1}, "x/y/f/g": {2}}, `"x/y/f" is both a file and a directory`},
	}
	for _, tt := range tests {
		f, _ := os.Create(filepath.Join(t.TempDir(), "x"))
		_, err := Write(f, 0, Volume{Size: 64 * mib, Time: fixedTime, Files: tt.files})
		f.Close()
		if err == nil || !strings.Contains(err.Error(), tt.want) {
			t.Errorf("%v: err = %v, want %q", keys(tt.files), err, tt.want)
		}
	}
	if err := checkLongName(strings.Repeat("é", 200)); err != nil {
		t.Errorf("200 characters of two bytes each are 200 UTF-16 units and should be allowed: %v", err)
	}
	if err := checkLongName(strings.Repeat("x", 255)); err != nil {
		t.Errorf("255 characters should be allowed: %v", err)
	}
	if err := checkLongName(strings.Repeat("🚀", 128)); err == nil {
		t.Error("128 rockets are 256 UTF-16 units and should be refused")
	}
}

func keys(m map[string][]byte) []string {
	var out []string
	for k := range m {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}

func TestFileSizeLimit(t *testing.T) {
	if err := checkFileSize("big", 0xFFFFFFFF); err != nil {
		t.Errorf("4 GiB minus one byte is allowed: %v", err)
	}
	if err := checkFileSize("big", 1<<32); err == nil || !strings.Contains(err.Error(), `"big" is 4294967296 bytes, FAT32 files end at 4 GiB`) {
		t.Errorf("4 GiB: %v", err)
	}
}

func TestDirectoryEntryLimit(t *testing.T) {
	files := map[string][]byte{}
	for i := range 65534 {
		files[fmt.Sprintf("D/F%05d", i)] = nil
	}
	f, _ := os.Create(filepath.Join(t.TempDir(), "x"))
	defer f.Close()
	if _, err := Write(f, 0, Volume{Size: 64 * mib, Time: fixedTime, Files: files}); err != nil {
		t.Fatalf("65536 entries including . and .. should fit: %v", err)
	}
	files["D/F65534"] = nil
	_, err := Write(f, 0, Volume{Size: 64 * mib, Time: fixedTime, Files: files})
	if err == nil || !strings.Contains(err.Error(), `directory "/D" needs 65537 entries, FAT allows 65536`) {
		t.Errorf("one entry too many: %v", err)
	}
}

func TestWriteAtAnOffset(t *testing.T) {
	const offset = mib
	img, _, _ := writeVolumeAt(t, sampleVolume(), offset)
	if !bytes.Equal(img[:offset], make([]byte, offset)) {
		t.Error("bytes before the offset were touched")
	}
	tr := openVolume(t, img[offset:]).walk()
	if !bytes.Equal(tr.files["meridian/kernel.elf"], sampleFiles()["meridian/kernel.elf"]) {
		t.Error("kernel.elf not readable at the offset")
	}
}

func TestFileTailsArePaddedWithZeros(t *testing.T) {
	path := filepath.Join(t.TempDir(), "dirty.img")
	if err := os.WriteFile(path, bytes.Repeat([]byte{0xEE}, 64*mib), 0o644); err != nil {
		t.Fatal(err)
	}
	f, _ := os.OpenFile(path, os.O_RDWR, 0)
	_, err := Write(f, 0, Volume{Size: 64 * mib, Time: fixedTime, Files: map[string][]byte{"A": {1, 2, 3}, "B/C": {4}, "ODD": pattern(511, 9)}})
	f.Close()
	if err != nil {
		t.Fatal(err)
	}
	img, _ := os.ReadFile(path)
	v := openVolume(t, img)
	tr := v.walk()
	zeroAfter := func(what string, first uint32, used int) {
		cluster := v.read(first, v.clusterBytes)
		if !bytes.Equal(cluster[used:], make([]byte, len(cluster)-used)) {
			t.Errorf("%s: bytes after offset %d still hold old data", what, used)
		}
	}
	for _, name := range []string{"A", "B/C", "ODD"} {
		zeroAfter(name, tr.ents[name].first, int(tr.ents[name].size))
	}
	// Root: "A", "B" and "ODD" with no label; B: ".", ".." and "C".
	zeroAfter("root", 2, 3*32)
	zeroAfter("B", tr.dirs["B"], 3*32)
}

func TestSameInputGivesTheSameVolume(t *testing.T) {
	a, _, _ := writeVolume(t, sampleVolume())
	b, _, _ := writeVolume(t, sampleVolume())
	if !bytes.Equal(a, b) {
		t.Fatal("two writes of the same volume differ")
	}
	v := sampleVolume()
	v.Time = fixedTime.Add(2 * time.Second)
	c, _, _ := writeVolume(t, v)
	if bytes.Equal(a, c) {
		t.Error("a different time should change the image")
	}
}

func TestWriterErrorsArePassedOn(t *testing.T) {
	path := filepath.Join(t.TempDir(), "ro.img")
	os.WriteFile(path, make([]byte, 64*mib), 0o644)
	f, _ := os.Open(path)
	defer f.Close()
	_, err := Write(f, 4096, sampleVolume())
	if err == nil || !strings.Contains(err.Error(), "fat: write at byte 4096") {
		t.Errorf("err = %v", err)
	}
}

func TestBadVolumeSettingsAreRefusedBeforeWriting(t *testing.T) {
	for name, v := range map[string]Volume{
		"label": {Size: 64 * mib, Time: fixedTime, Label: "far too long label"},
		"time":  {Size: 64 * mib, Time: time.Time{}},
		"size":  {Size: 1000, Time: fixedTime},
		"path":  {Size: 64 * mib, Time: fixedTime, Files: map[string][]byte{"a|b": nil}},
	} {
		path := filepath.Join(t.TempDir(), "x")
		f, _ := os.Create(path)
		_, err := Write(f, 0, v)
		f.Close()
		info, _ := os.Stat(path)
		if err == nil || info.Size() != 0 {
			t.Errorf("%s: err %v, wrote %d bytes", name, err, info.Size())
		}
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

// fsck.fat from dosfstools checks the whole volume on its own terms.
func TestFsckFatFindsNothingWrong(t *testing.T) {
	fsck := requireTool(t, "fsck.fat")
	_, res, path := writeVolume(t, sampleVolume())
	out, err := exec.Command(fsck, "-n", "-v", path).CombinedOutput()
	if err != nil {
		t.Fatalf("fsck.fat -n -v: %v\n%s", err, out)
	}
	text := string(out)
	m := regexp.MustCompile(`(\d+) files, (\d+)/(\d+) clusters`).FindStringSubmatch(text)
	if m == nil {
		t.Fatalf("no summary line in:\n%s", text)
	}
	used, _ := strconv.Atoi(m[2])
	total, _ := strconv.Atoi(m[3])
	if uint32(used) != res.UsedClusters || uint32(total) != res.Clusters {
		t.Errorf("fsck.fat counts %d/%d clusters, the writer %d/%d", used, total, res.UsedClusters, res.Clusters)
	}
	for _, bad := range []string{"differ", "Free cluster summary wrong", "Dirty bit", "Bad short file name", "invalid", "corrected"} {
		if strings.Contains(text, bad) {
			t.Errorf("fsck.fat reports %q:\n%s", bad, text)
		}
	}
}

func TestFatlabelReadsTheLabel(t *testing.T) {
	fatlabel := requireTool(t, "fatlabel")
	_, _, path := writeVolume(t, sampleVolume())
	out, err := exec.Command(fatlabel, path).CombinedOutput()
	if err != nil || strings.TrimSpace(string(out)) != "MERIDIAN" {
		t.Errorf("fatlabel: %q, %v", out, err)
	}
}

// mtools lists and reads the volume with its own FAT driver, long names
// included.
func TestMtoolsReadsEveryFile(t *testing.T) {
	mdir := requireTool(t, "mdir")
	mcopy := requireTool(t, "mcopy")
	const offset = mib
	_, _, path := writeVolumeAt(t, sampleVolume(), offset)
	image := path + "@@" + strconv.Itoa(offset)
	env := append(os.Environ(), "MTOOLS_SKIP_CHECK=1", "LC_ALL=C.UTF-8")

	cmd := exec.Command(mdir, "-/", "-b", "-i", image, "::")
	cmd.Env = env
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("mdir: %v\n%s", err, out)
	}
	listed := map[string]bool{}
	for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		listed[strings.TrimPrefix(strings.TrimSpace(line), "::/")] = true
	}
	for p, data := range sampleFiles() {
		if !listed[p] {
			t.Errorf("mdir does not list %q; it lists:\n%s", p, out)
			continue
		}
		cp := exec.Command(mcopy, "-n", "-i", image, "::/"+p, "-")
		cp.Env = env
		got, err := cp.Output()
		if err != nil {
			t.Errorf("mcopy %q: %v", p, err)
			continue
		}
		if !bytes.Equal(got, data) {
			t.Errorf("mcopy reads %d bytes from %q, wrote %d", len(got), p, len(data))
		}
	}
}

func TestEmptyVolumeStillHasARootCluster(t *testing.T) {
	img, res, path := writeVolume(t, Volume{Size: 64 * mib, Time: fixedTime})
	v := openVolume(t, img)
	if chain := v.chain(2); len(chain) != 1 || res.UsedClusters != 1 {
		t.Errorf("root chain %v, %d clusters used; want one cluster", chain, res.UsedClusters)
	}
	if fsck, err := exec.LookPath("fsck.fat"); err == nil {
		if out, err := exec.Command(fsck, "-n", path).CombinedOutput(); err != nil {
			t.Errorf("fsck.fat on an empty volume: %v\n%s", err, out)
		}
	}
}

// Cluster numbers above 65535 need the high word of the first-cluster field,
// so a file that starts past the first 32 MiB catches a wrong split.
func TestFileStartingAboveCluster65535(t *testing.T) {
	files := map[string][]byte{"A": make([]byte, 34*mib), "B": pattern(700, 11)}
	img, _, _ := writeVolume(t, Volume{Size: 64 * mib, Time: fixedTime, Files: files})
	tr := openVolume(t, img).walk()
	if first := tr.ents["B"].first; first <= 0xFFFF {
		t.Fatalf("B starts at cluster %d, the test needs it above 65535", first)
	}
	if !bytes.Equal(tr.files["B"], files["B"]) {
		t.Error("B reads back wrong")
	}
}

// With 4 KiB clusters the shortfall is counted in 4 KiB steps.
func TestShortfallUsesTheRealClusterSize(t *testing.T) {
	size := int64(532481 * 512)
	g, err := planGeometry(size)
	if err != nil || g.sectorsPerCluster != 8 {
		t.Fatalf("geometry %+v, %v", g, err)
	}
	// Every directory takes one cluster; split them over two parents so no
	// directory holds more than 65536 entries.
	files := map[string][]byte{}
	for i := range int(g.clusters) {
		files[fmt.Sprintf("P%d/D%05d/F", i%2, i/2)] = nil
	}
	f, _ := os.Create(filepath.Join(t.TempDir(), "x"))
	defer f.Close()
	_, err = Write(f, 0, Volume{Size: size, Time: fixedTime, Files: files})
	if err == nil {
		t.Fatal("more directories than clusters were accepted")
	}
	m := regexp.MustCompile(`need (\d+) clusters of 4096 bytes but the volume has (\d+); it is (\d+) bytes`).FindStringSubmatch(err.Error())
	if m == nil {
		t.Fatalf("err = %v", err)
	}
	need, _ := strconv.Atoi(m[1])
	have, _ := strconv.Atoi(m[2])
	short, _ := strconv.Atoi(m[3])
	if uint32(have) != g.clusters || short != (need-have)*4096 {
		t.Errorf("need %d have %d short %d", need, have, short)
	}
}
