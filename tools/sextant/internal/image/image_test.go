package image

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/binary"
	"encoding/json"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/meri"
)

var stamp = time.Date(2026, 10, 14, 5, 0, 0, 0, time.UTC)

func tool(t *testing.T, name string) string {
	t.Helper()
	path, err := exec.LookPath(name)
	if err != nil {
		if os.Getenv("SEXTANT_REQUIRE_TOOLS") == "1" {
			t.Fatalf("%s is required but missing", name)
		}
		t.Skipf("%s is missing", name)
	}
	return path
}

func files() map[string][]byte {
	return map[string][]byte{
		"EFI/BOOT/BOOTX64.EFI": bytes.Repeat([]byte("MZloader"), 4096),
		"meridian/boot.meri":   []byte("timeout = 3s\n"),
	}
}

func diskFile(t *testing.T, c Contents) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "meridian.img")
	if err := Disk(path, c); err != nil {
		t.Fatal(err)
	}
	return path
}

func sum(t *testing.T, path string) [32]byte {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return sha256.Sum256(data)
}

func bootParts() Boot {
	return Boot{
		Loader: []byte("efi"), Certificate: []byte("cer"), Kernel: []byte("elf"),
		Shim:         Shim{Shim: []byte("shim"), Manager: []byte("mok"), Licences: []byte("bsd")},
		FontLicences: map[string][]byte{"inter.txt": []byte("ofl"), "jetbrains_mono.txt": []byte("ofl too")},
		Programs:     map[string][]byte{"hello.mexec": []byte("hi"), "init.mexec": []byte("first")},
	}
}

func TestBootFilesGoWhereFirmwareShimAndLoaderLook(t *testing.T) {
	got := BootFiles(bootParts())
	want := map[string]string{
		"EFI/BOOT/BOOTX64.EFI":                 "shim",
		"EFI/BOOT/grubx64.efi":                 "efi",
		"EFI/BOOT/mmx64.efi":                   "mok",
		"meridian.cer":                         "cer",
		"meridian/licences/shim.txt":           "bsd",
		"meridian/licences/inter.txt":          "ofl",
		"meridian/licences/jetbrains_mono.txt": "ofl too",
		"meridian/kernel":                      "elf",
		"meridian/boot.meri":                   Settings,
		"meridian/services.meri":               Services,
		"meridian/programs/hello.mexec":        "hi",
		"meridian/programs/init.mexec":         "first",
	}
	if len(got) != len(want) {
		t.Fatalf("got %d files, want %d", len(got), len(want))
	}
	for path, w := range want {
		if string(got[path]) != w {
			t.Errorf("%s holds %q, want %q", path, got[path], w)
		}
	}
}

func TestTestImagesCarryAnExampleSystem(t *testing.T) {
	got := TestFiles(bootParts())
	if len(got) != len(BootFiles(bootParts()))+5 || len(LongName) != 254 || string(got[LongName]) != "long name\n" ||
		string(got["meridian/programs/hello_again.mexec"]) != "hi" || string(got["meridian/programs/init_again.mexec"]) != "first" ||
		string(got["EFI/example/grubx64.efi"]) != "efi" || string(got["EFI/other/tool.efi"]) != "efi" ||
		string(got[KernelPath]) != "elf" || string(got[LoaderPath]) != "efi" || string(got[FirmwarePath]) != "shim" {
		t.Fatalf("got %v", got)
	}
	if string(got[SettingsPath]) != testSettings || string(got[ServicesPath]) != testServices {
		t.Fatal("test images carry settings and services of their own")
	}
}

func TestTheFontLicencesComeFromTheRepository(t *testing.T) {
	licences, err := ReadFontLicences(filepath.Join("..", "..", "..", ".."))
	if err != nil {
		t.Fatal(err)
	}
	if len(licences) != 2 {
		t.Fatalf("got %d licences", len(licences))
	}
	for name, font := range map[string]string{"inter.txt": "The Inter Project Authors", "jetbrains_mono.txt": "The JetBrains Mono Project Authors"} {
		text := licences[name]
		if !bytes.Contains(text, []byte("SIL OPEN FONT LICENSE Version 1.1")) || !bytes.Contains(text, []byte(font)) {
			t.Errorf("%s should be the Open Font License of %s: %.80q", name, font, text)
		}
	}
	_, err = ReadFontLicences(t.TempDir())
	if !errors.Is(err, os.ErrNotExist) || !strings.Contains(err.Error(), "decision record 0012") {
		t.Errorf("a repository without the fonts: %v", err)
	}
}

func TestTheShimComesFromTheRepository(t *testing.T) {
	s, err := ReadShim(filepath.Join("..", "..", "..", ".."))
	if err != nil {
		t.Fatal(err)
	}
	for name, b := range map[string][]byte{"the shim": s.Shim, "MokManager": s.Manager} {
		if len(b) < 64<<10 || string(b[:2]) != "MZ" {
			t.Errorf("%s is no EFI application: %d bytes", name, len(b))
		}
	}
	if bytes.Equal(s.Shim, s.Manager) {
		t.Error("the shim and MokManager are different files")
	}
	if !bytes.Contains(s.Licences, []byte("License: OpenSSL")) || !bytes.Contains(s.Licences, []byte("BSD-2-Clause")) {
		t.Error("the licences should be the source package's copyright file")
	}
	_, err = ReadShim(t.TempDir())
	if !errors.Is(err, os.ErrNotExist) || !strings.Contains(err.Error(), "decision record 0013") {
		t.Errorf("a repository without the shim: %v", err)
	}
}

// The settings files are read with sextant's own .meri reader, which
// passes the same conformance suite as the loader's.
func TestTheSettingsFilesSayWhatTheyShould(t *testing.T) {
	cases := []struct {
		name, src string
		timeout   time.Duration
		start     string
		verbose   bool
	}{
		{"boot.meri", Settings, 3 * time.Second, "meridian", false},
		{"test boot.meri", testSettings, 2500 * time.Millisecond, "example", true},
	}
	for _, c := range cases {
		doc, err := meri.Parse(c.name, []byte(c.src))
		if err != nil {
			t.Fatalf("%s: %v", c.name, err)
		}
		blocks := doc.Blocks("boot")
		if len(blocks) != 1 {
			t.Fatalf("%s: %d boot blocks", c.name, len(blocks))
		}
		boot := blocks[0]
		timeout, err1 := boot.Duration("timeout", 0)
		start, err2 := boot.String("default", "")
		verbose, err3 := boot.Bool("verbose", !c.verbose)
		if err := errors.Join(err1, err2, err3); err != nil {
			t.Fatalf("%s: %v", c.name, err)
		}
		if timeout != c.timeout || start != c.start || verbose != c.verbose {
			t.Errorf("%s: timeout %v, default %q, verbose %v", c.name, timeout, start, verbose)
		}
	}
}

// init is a Rust program, so this reads its source to make sure it reads
// the list where BootFiles puts it.
func TestInitReadsTheServicesWhereTheyGo(t *testing.T) {
	src, err := os.ReadFile(filepath.Join("..", "..", "..", "..", "lib", "program", "examples", "init.rs"))
	if err != nil {
		t.Fatal(err)
	}
	want := `const LIST: &str = "/System/Boot/` + ServicesPath + `";`
	if !strings.Contains(string(src), want) {
		t.Errorf("init.rs should say %s", want)
	}
}

func TestTheServicesListsNameTheirPrograms(t *testing.T) {
	for _, c := range []struct {
		name, src string
		// Each service's name and the program it runs.
		services [][2]string
	}{
		{"services.meri", Services, [][2]string{{"shell", "shell"}}},
		{"test services.meri", testServices, [][2]string{{"setup", "whoami"}, {"whoami", "whoami"}, {"shell", "shell"}}},
	} {
		doc, err := meri.Parse(c.name, []byte(c.src))
		if err != nil {
			t.Fatalf("%s: %v", c.name, err)
		}
		blocks := doc.Blocks("service")
		if len(blocks) != len(c.services) {
			t.Fatalf("%s: the services are %v", c.name, blocks)
		}
		for i, b := range blocks {
			program, err := b.String("program", "")
			if !b.Labelled || b.Label != c.services[i][0] || err != nil || program != c.services[i][1] {
				t.Errorf("%s: service %q runs %q, %v", c.name, b.Label, program, err)
			}
		}
	}
}

func TestTheTestServicesRunOneOnceAndWaitForIt(t *testing.T) {
	doc, err := meri.Parse("test services.meri", []byte(testServices))
	if err != nil {
		t.Fatal(err)
	}
	once := map[string]bool{}
	after := map[string]string{}
	for _, b := range doc.Blocks("service") {
		if once[b.Label], err = b.Bool("once", false); err != nil {
			t.Fatal(err)
		}
		if after[b.Label], err = b.String("after", ""); err != nil {
			t.Fatal(err)
		}
	}
	if !once["setup"] || once["whoami"] || once["shell"] {
		t.Errorf("only setup should run once: %v", once)
	}
	if after["whoami"] != "setup" || after["shell"] != "whoami" || after["setup"] != "" {
		t.Errorf("whoami should wait for setup, and the shell for whoami: %v", after)
	}
}

// The loader is written in Rust, so this reads its source to make sure it
// looks for the kernel and the settings where BootFiles puts them, and
// knows where it and the shim are.
func TestTheLoaderLooksWhereTheKernelGoes(t *testing.T) {
	for _, c := range []struct{ file, want string }{
		{"lib.rs", `pub const KERNEL_PATH: &str = "/` + KernelPath + `";`},
		{"lib.rs", `pub const LOADER_PATH: &str = "/` + LoaderPath + `";`},
		{"lib.rs", `pub const FIRMWARE_PATH: &str = "/` + FirmwarePath + `";`},
		{"settings.rs", `pub const PATH: &str = "/` + SettingsPath + `";`},
		{"lib.rs", `pub const PROGRAMS_PATH: &str = "/` + ProgramsPath + `";`},
	} {
		src, err := os.ReadFile(filepath.Join("..", "..", "..", "..", "boot", "src", c.file))
		if err != nil {
			t.Fatal(err)
		}
		if !strings.Contains(string(src), c.want) {
			t.Errorf("boot/src/%s should say %s", c.file, c.want)
		}
	}
}

func TestDiskIsReproducible(t *testing.T) {
	c := Contents{Seed: "meridian 0.0.0", Time: stamp, Files: files()}
	a, b := diskFile(t, c), diskFile(t, c)
	if sum(t, a) != sum(t, b) {
		t.Fatal("the same contents must give the same bytes")
	}
	c.Seed = "meridian 0.0.1"
	if sum(t, diskFile(t, c)) == sum(t, a) {
		t.Fatal("a different seed must change the image")
	}
	info, err := os.Stat(a)
	if err != nil {
		t.Fatal(err)
	}
	if info.Size() != ESPSize+2<<20 {
		t.Fatalf("size %d", info.Size())
	}
}

func TestDiskLayoutAsSfdiskSeesIt(t *testing.T) {
	sfdisk := tool(t, "sfdisk")
	path := diskFile(t, Contents{Seed: "layout", Time: stamp, Files: files()})
	out, err := exec.Command(sfdisk, "--json", path).Output()
	if err != nil {
		t.Fatalf("sfdisk: %v", err)
	}
	var table struct {
		PartitionTable struct {
			Label      string `json:"label"`
			Partitions []struct {
				Start int64  `json:"start"`
				Size  int64  `json:"size"`
				Type  string `json:"type"`
				Name  string `json:"name"`
			} `json:"partitions"`
		} `json:"partitiontable"`
	}
	if err := json.Unmarshal(out, &table); err != nil {
		t.Fatalf("%v\n%s", err, out)
	}
	pt := table.PartitionTable
	if pt.Label != "gpt" || len(pt.Partitions) != 1 {
		t.Fatalf("table: %+v", pt)
	}
	p := pt.Partitions[0]
	if p.Start != 2048 || p.Size != ESPSize/512 || p.Type != "C12A7328-F81F-11D2-BA4B-00A0C93EC93B" || p.Name != "EFI system partition" {
		t.Fatalf("partition: %+v", p)
	}
	if out, err := exec.Command(sfdisk, "--verify", path).CombinedOutput(); err != nil {
		t.Fatalf("sfdisk --verify: %v\n%s", err, out)
	}
}

// sfdiskPartition is a partition as sfdisk --json describes it, in
// sectors.
type sfdiskPartition struct {
	Start int64  `json:"start"`
	Size  int64  `json:"size"`
	Type  string `json:"type"`
	UUID  string `json:"uuid"`
	Name  string `json:"name"`
}

func partitionsOf(t *testing.T, path string) []sfdiskPartition {
	t.Helper()
	out, err := exec.Command(tool(t, "sfdisk"), "--json", path).Output()
	if err != nil {
		t.Fatalf("sfdisk: %v", err)
	}
	var table struct {
		PartitionTable struct {
			Partitions []sfdiskPartition `json:"partitions"`
		} `json:"partitiontable"`
	}
	if err := json.Unmarshal(out, &table); err != nil {
		t.Fatalf("%v\n%s", err, out)
	}
	return table.PartitionTable.Partitions
}

func testDisk(t *testing.T) string {
	t.Helper()
	return diskFile(t, Contents{Seed: "system", Time: stamp, Files: files(), System: TestSystem, Others: TestOthers})
}

func TestATestDiskHasMeridiansSystemAndPartitionsOfOthers(t *testing.T) {
	sfdisk := tool(t, "sfdisk")
	path := testDisk(t)
	parts := partitionsOf(t, path)
	if len(parts) != 4 || parts[0].Size != ESPSize/512 {
		t.Fatalf("partitions: %+v", parts)
	}
	want := []struct {
		typ, name string
		size      int64
	}{
		{"B0DD29AB-2286-43EF-9809-E345D01A1B29", "Meridian system", TestSystem / 512},
		{"EBD0A0A2-B9E5-4433-87C0-68B6B72699C7", "Unlocked", 2048},
		{"B0DD29AB-2286-43EF-9809-E345D01A1B29", "Not listed", 2048},
	}
	for i, w := range want {
		p, before := parts[i+1], parts[i]
		if p.Type != w.typ || p.Name != w.name || p.Size != w.size || p.Start < before.Start+before.Size {
			t.Fatalf("partition %d: %+v", i+2, p)
		}
	}
	if out, err := exec.Command(sfdisk, "--verify", path).CombinedOutput(); err != nil {
		t.Fatalf("sfdisk --verify: %v\n%s", err, out)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	for _, p := range parts[2:] {
		room := data[p.Start*512 : (p.Start+p.Size)*512]
		if !bytes.Equal(room, make([]byte, len(room))) {
			t.Errorf("%s should be left empty", p.Name)
		}
	}
}

func TestTheSystemPartitionListsWhatMeridianMayWrite(t *testing.T) {
	mtype := tool(t, "mtype")
	fsck := tool(t, "fsck.fat")
	path := testDisk(t)
	parts := partitionsOf(t, path)
	if len(parts) != 4 {
		t.Fatalf("partitions: %+v", parts)
	}
	system := parts[1]
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	volume := filepath.Join(t.TempDir(), "system.img")
	if err := os.WriteFile(volume, data[system.Start*512:(system.Start+system.Size)*512], 0o644); err != nil {
		t.Fatal(err)
	}
	if out, err := exec.Command(fsck, "-n", volume).CombinedOutput(); err != nil {
		t.Fatalf("fsck.fat: %v\n%s", err, out)
	}
	if hidden := binary.LittleEndian.Uint32(data[system.Start*512+28:]); int64(hidden) != system.Start {
		t.Fatalf("hidden sectors = %d, want %d", hidden, system.Start)
	}
	listed := func(name string) []*meri.Block {
		out, err := exec.Command(mtype, "-i", volume, "::/"+name).Output()
		if err != nil {
			t.Fatalf("mtype %s: %v", name, err)
		}
		root, err := meri.Parse(name, out)
		if err != nil {
			t.Fatalf("%s: %v\n%s", name, err, out)
		}
		return root.Blocks("partition")
	}
	check := func(file string, blocks []*meri.Block, want []sfdiskPartition) {
		t.Helper()
		if len(blocks) != len(want) {
			t.Fatalf("%s lists %d partitions, want %d", file, len(blocks), len(want))
		}
		for i, b := range blocks {
			name, _ := b.String("name", "")
			size, _ := b.Size("size", 0)
			if !b.Labelled || !strings.EqualFold(b.Label, want[i].UUID) || name != want[i].Name || size != want[i].Size*512 {
				t.Errorf("%s, partition %d: %q %q %d, want %+v", file, i, b.Label, name, size, want[i])
			}
		}
	}
	check(MadePath, listed(MadePath), parts[:2])
	check(UnlockedPath, listed(UnlockedPath), parts[2:3])
}

func TestAPlainDiskListsNothing(t *testing.T) {
	path := diskFile(t, Contents{Seed: "plain", Time: stamp, Files: files(), Others: TestOthers[:1]})
	parts := partitionsOf(t, path)
	if len(parts) != 2 || parts[1].Name != "Unlocked" || parts[1].Start != parts[0].Start+parts[0].Size {
		t.Fatalf("partitions: %+v", parts)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	room := data[parts[1].Start*512 : (parts[1].Start+parts[1].Size)*512]
	if !bytes.Equal(room, make([]byte, len(room))) {
		t.Error("without a system partition there is no list, and the other partition stays empty")
	}
}

func TestNamesAndSizesReadBackAsTheyWereWritten(t *testing.T) {
	for _, name := range []string{"", "Data", `a " and a \`, "tab\tline\ncr\r", "del\x7f nul\x00 esc\x1b", "Größe 😀"} {
		src := "name = " + meriString(name) + "\n"
		root, err := meri.Parse("name", []byte(src))
		if err != nil {
			t.Fatalf("%q: %v\n%s", name, err, src)
		}
		if got, _ := root.String("name", "missing"); got != name {
			t.Errorf("%q read back as %q", name, got)
		}
	}
	for size, want := range map[int64]string{
		0: "0B", 1: "1B", 512: "512B", 1536: "1536B", 1024: "1KiB", 3 << 10: "3KiB",
		1 << 20: "1MiB", 1<<20 + 512: "1049088B", 34 << 20: "34MiB", 5 << 30: "5GiB",
		1<<40 + 1<<30: "1025GiB", 7 << 40: "7TiB",
	} {
		if got := meriSize(size); got != want {
			t.Errorf("%d bytes written as %s, want %s", size, got, want)
		}
		root, err := meri.Parse("size", []byte("size = "+meriSize(size)+"\n"))
		if err != nil {
			t.Fatal(err)
		}
		if got, _ := root.Size("size", -1); got != size {
			t.Errorf("%s read back as %d bytes, want %d", meriSize(size), got, size)
		}
	}
}

func TestDiskFilesAsMtoolsSeesThem(t *testing.T) {
	mtype := tool(t, "mtype")
	fsck := tool(t, "fsck.fat")
	path := diskFile(t, Contents{Seed: "files", Time: stamp, Files: files()})
	at := path + "@@1M"
	for name, want := range files() {
		out, err := exec.Command(mtype, "-i", at, "::/"+name).Output()
		if err != nil {
			t.Fatalf("mtype %s: %v", name, err)
		}
		if !bytes.Equal(out, want) {
			t.Fatalf("%s holds %d bytes, want %d", name, len(out), len(want))
		}
	}
	esp := filepath.Join(t.TempDir(), "esp.img")
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(esp, data[1<<20:1<<20+ESPSize], 0o644); err != nil {
		t.Fatal(err)
	}
	if out, err := exec.Command(fsck, "-n", esp).CombinedOutput(); err != nil {
		t.Fatalf("fsck.fat: %v\n%s", err, out)
	}
	// BPB_HiddSec at offset 28 of the boot sector: the sectors before the
	// partition, which some firmware uses to find the volume on the disk.
	if hidden := binary.LittleEndian.Uint32(data[1<<20+28:]); hidden != 2048 {
		t.Fatalf("hidden sectors = %d, want 2048", hidden)
	}
}

func TestISOAsXorrisoSeesIt(t *testing.T) {
	xorriso := tool(t, "xorriso")
	path := filepath.Join(t.TempDir(), "meridian.iso")
	if err := ISO(path, Contents{Seed: "iso", Time: stamp, Files: files()}); err != nil {
		t.Fatal(err)
	}
	out, err := exec.Command(xorriso, "-indev", path, "-report_el_torito", "plain").CombinedOutput()
	if err != nil {
		t.Fatalf("xorriso: %v\n%s", err, out)
	}
	if !strings.Contains(string(out), "UEFI") {
		t.Fatalf("no UEFI boot entry in:\n%s", out)
	}
	// xorriso's extraction guesses the size of an image whose sector count
	// is 0 and comes out one block short, so the partition is cut out of
	// the ISO at the catalog's LBA instead.
	lba := -1
	for _, line := range strings.Split(string(out), "\n") {
		if fields := strings.Fields(line); strings.HasPrefix(line, "El Torito boot img") && len(fields) > 0 {
			lba, err = strconv.Atoi(fields[len(fields)-1])
			if err != nil {
				t.Fatalf("no LBA in %q", line)
			}
		}
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	start := int64(lba) * 2048
	if lba < 0 || int64(len(data)) < start+ESPSize {
		t.Fatalf("the ISO is %d bytes, too short for the partition at LBA %d", len(data), lba)
	}
	esp := filepath.Join(t.TempDir(), "esp.img")
	if err := os.WriteFile(esp, data[start:start+ESPSize], 0o644); err != nil {
		t.Fatal(err)
	}
	if out, err := exec.Command(tool(t, "fsck.fat"), "-n", esp).CombinedOutput(); err != nil {
		t.Fatalf("the boot entry does not point at a clean FAT volume: %v\n%s", err, out)
	}
	again := filepath.Join(t.TempDir(), "again.iso")
	if err := ISO(again, Contents{Seed: "iso", Time: stamp, Files: files()}); err != nil {
		t.Fatal(err)
	}
	if sum(t, path) != sum(t, again) {
		t.Fatal("the ISO must be reproducible too")
	}
}

func TestATestImageMakesAnISO(t *testing.T) {
	files := TestFiles(bootParts())
	cd := onCD(files)
	if _, ok := cd[LongName]; ok || len(cd) != len(files)-1 {
		t.Fatalf("on the CD: %d of %d files, the long name among them: %v", len(cd), len(files), ok)
	}
	for p, data := range cd {
		if string(files[p]) != string(data) {
			t.Errorf("%s changed on the way to the CD", p)
		}
	}
	if err := ISO(filepath.Join(t.TempDir(), "test.iso"), Contents{Seed: "iso", Time: stamp, Files: files}); err != nil {
		t.Fatal(err)
	}
}

func TestErrorsFromTheWriters(t *testing.T) {
	missingDir := filepath.Join(t.TempDir(), "no", "such", "dir", "x.img")
	if err := Disk(missingDir, Contents{Seed: "x", Time: stamp}); err == nil {
		t.Error("a path that cannot be created must fail")
	}
	if err := ISO(missingDir, Contents{Seed: "x", Time: stamp}); err == nil {
		t.Error("an ISO path that cannot be created must fail")
	}
	tooBig := map[string][]byte{"big.bin": make([]byte, ESPSize)}
	if err := Disk(filepath.Join(t.TempDir(), "big.img"), Contents{Seed: "x", Time: stamp, Files: tooBig}); err == nil || !strings.HasPrefix(err.Error(), "EFI system partition: ") {
		t.Errorf("a file larger than the partition: %v", err)
	}
	if err := ISO(filepath.Join(t.TempDir(), "big.iso"), Contents{Seed: "x", Time: stamp, Files: tooBig}); err == nil || !strings.HasPrefix(err.Error(), "EFI system partition: ") {
		t.Errorf("a file larger than the ISO's partition: %v", err)
	}
	// FAT's clock starts in 1980 and is clamped; ISO 9660 refuses what it
	// cannot store, after the file was made.
	early := time.Date(1899, 12, 31, 0, 0, 0, 0, time.UTC)
	if err := ISO(filepath.Join(t.TempDir(), "early.iso"), Contents{Seed: "x", Time: early, Files: files()}); err == nil || !strings.Contains(err.Error(), "between 1900 and 2155") {
		t.Errorf("a time before ISO 9660's: %v", err)
	}
}

func TestTimesBeforeFATsEpochAreClamped(t *testing.T) {
	if got := fatTime(time.Unix(0, 0)); !got.Equal(fatEpoch) {
		t.Errorf("1970 became %v", got)
	}
	if got := fatTime(fatEpoch.Add(-time.Second)); !got.Equal(fatEpoch) {
		t.Errorf("one second early became %v", got)
	}
	if got := fatTime(stamp); !got.Equal(stamp) {
		t.Errorf("a normal time changed to %v", got)
	}
	if err := Disk(filepath.Join(t.TempDir(), "epoch.img"), Contents{Seed: "x", Time: time.Unix(0, 0), Files: files()}); err != nil {
		t.Errorf("SOURCE_DATE_EPOCH=0 must still give an image: %v", err)
	}
}

func TestVolumeIDsComeFromTheSeed(t *testing.T) {
	if volumeID("a") == volumeID("b") || volumeID("a") != volumeID("a") {
		t.Fatal("volume IDs must be stable per seed and differ between seeds")
	}
}

func TestBuildTimeFromTheEnvironment(t *testing.T) {
	env := func(v string) func(string) string {
		return func(k string) string {
			if k == "SOURCE_DATE_EPOCH" {
				return v
			}
			return ""
		}
	}
	got, err := BuildTime(context.Background(), t.TempDir(), env("1760418000"))
	if err != nil || !got.Equal(time.Unix(1760418000, 0)) || got.Location() != time.UTC {
		t.Fatalf("got %v, %v", got, err)
	}
	if _, err := BuildTime(context.Background(), t.TempDir(), env("yesterday")); err == nil || err.Error() != "SOURCE_DATE_EPOCH must be seconds since 1970, got \"yesterday\"" {
		t.Fatalf("got %v", err)
	}
}

func TestBuildTimeFromGit(t *testing.T) {
	git := tool(t, "git")
	dir := t.TempDir()
	run := func(args ...string) {
		cmd := exec.Command(git, append([]string{"-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false"}, args...)...)
		cmd.Dir = dir
		cmd.Env = append(os.Environ(), "GIT_COMMITTER_DATE=2026-10-14T05:00:00Z", "GIT_AUTHOR_DATE=2026-10-14T05:00:00Z")
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	run("init", "-q")
	if err := os.WriteFile(filepath.Join(dir, "f"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	run("add", "f")
	run("commit", "-q", "-m", "x")
	got, err := BuildTime(context.Background(), dir, func(string) string { return "" })
	if err != nil || !got.Equal(stamp) {
		t.Fatalf("got %v, %v", got, err)
	}
	if _, err := BuildTime(context.Background(), t.TempDir(), func(string) string { return "" }); err == nil || !strings.HasPrefix(err.Error(), "read the commit time with git: ") {
		t.Fatalf("outside a repository: %v", err)
	}
}
