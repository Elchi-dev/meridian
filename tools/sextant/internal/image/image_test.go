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

func TestBootFilesGoWhereFirmwareAndLoaderLook(t *testing.T) {
	got := BootFiles([]byte("efi"), []byte("elf"))
	if len(got) != 3 || string(got["EFI/BOOT/BOOTX64.EFI"]) != "efi" || string(got["meridian/kernel"]) != "elf" ||
		string(got["meridian/boot.meri"]) != Settings {
		t.Fatalf("got %v", got)
	}
}

func TestTestImagesCarryAnExampleSystem(t *testing.T) {
	got := TestFiles([]byte("efi"), []byte("elf"))
	if len(got) != 5 || string(got["EFI/example/grubx64.efi"]) != "efi" || string(got["EFI/other/tool.efi"]) != "efi" ||
		string(got[KernelPath]) != "elf" || string(got[LoaderPath]) != "efi" {
		t.Fatalf("got %v", got)
	}
	if string(got[SettingsPath]) != testSettings {
		t.Fatal("test images carry settings of their own")
	}
	if len(BootFiles([]byte("efi"), []byte("elf"))) != 3 {
		t.Fatal("plain images carry no example")
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

// The loader is written in Rust, so this reads its source to make sure it
// looks for the kernel and the settings where BootFiles puts them.
func TestTheLoaderLooksWhereTheKernelGoes(t *testing.T) {
	for file, want := range map[string]string{
		"lib.rs":      `pub const KERNEL_PATH: &str = "/` + KernelPath + `";`,
		"settings.rs": `pub const PATH: &str = "/` + SettingsPath + `";`,
	} {
		src, err := os.ReadFile(filepath.Join("..", "..", "..", "..", "boot", "src", file))
		if err != nil {
			t.Fatal(err)
		}
		if !strings.Contains(string(src), want) {
			t.Errorf("boot/src/%s should say %s", file, want)
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
