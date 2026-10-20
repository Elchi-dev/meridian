// Package image assembles bootable Meridian images: a GPT disk with an EFI
// system partition, or an ISO that boots the same partition from CD.
package image

import (
	"context"
	"errors"
	"fmt"
	"hash/crc32"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/disk/fat"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/disk/gpt"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/disk/iso"
)

// ESPSize is the size of the EFI system partition. FAT32 needs at least
// about 32.5MiB; 64MiB leaves room for the kernel and its files.
const ESPSize = 64 << 20

// Contents is what goes onto an image.
type Contents struct {
	// Seed makes the image's GUIDs and volume serial numbers. The same seed
	// and files give a byte-identical image.
	Seed  string
	Time  time.Time
	Files map[string][]byte
}

// FirmwarePath is where UEFI firmware looks for a loader on removable
// media and on disks without boot entries. Images put the shim there,
// which starts Meridian's loader (decision record 0013).
const FirmwarePath = "EFI/BOOT/BOOTX64.EFI"

// LoaderPath is where the shim looks for what it starts next: Meridian's
// loader. Every shim has that name built in.
const LoaderPath = "EFI/BOOT/grubx64.efi"

// ManagerPath is MokManager, which the shim starts when it cannot check
// the loader, so the person can enroll the loader's key.
const ManagerPath = "EFI/BOOT/mmx64.efi"

// CertificatePath is the certificate of the key the loader is signed
// with, in DER, for the person to pick in MokManager. At the top of the
// partition it is there as soon as they pick the disk.
const CertificatePath = "meridian.cer"

// ShimLicencesPath carries the licences of the shim and MokManager along
// with them, as those licences ask.
const ShimLicencesPath = LicencesPath + "/shim.txt"

// ShimDir is where the repository keeps the shim, MokManager and their
// licences.
const ShimDir = "third-party/shim"

// Shim is the first stage of every image and what comes with it.
type Shim struct {
	Shim, Manager, Licences []byte
}

// ReadShim reads the shim, MokManager and their licences from ShimDir in
// the repository at root.
func ReadShim(root string) (Shim, error) {
	var s Shim
	for _, f := range []struct {
		name string
		into *[]byte
	}{{"shimx64.efi", &s.Shim}, {"mmx64.efi", &s.Manager}, {"COPYRIGHT", &s.Licences}} {
		data, err := os.ReadFile(filepath.Join(root, ShimDir, f.name))
		if err != nil {
			return Shim{}, fmt.Errorf("the shim, as decision record 0013 lists it: %w", err)
		}
		*f.into = data
	}
	return s, nil
}

// FontDir is where the repository keeps the fonts the loader and the
// kernel carry, and their licences.
const FontDir = "third-party/fonts"

// FontLicences are those licences, which images carry in LicencesPath
// since the fonts' licence has to travel with them (decision record
// 0012): by the name on the image, which a CD can carry too, the file in
// FontDir.
var FontLicences = map[string]string{"inter.txt": "Inter-OFL.txt", "jetbrains_mono.txt": "JetBrainsMono-OFL.txt"}

// LicencesPath is the folder of the licences an image carries.
const LicencesPath = "meridian/licences"

// ReadFontLicences reads the fonts' licences from FontDir in the
// repository at root, by their names on the image.
func ReadFontLicences(root string) (map[string][]byte, error) {
	licences := map[string][]byte{}
	for name, file := range FontLicences {
		data, err := os.ReadFile(filepath.Join(root, FontDir, file))
		if err != nil {
			return nil, fmt.Errorf("the fonts' licences, as decision record 0012 lists them: %w", err)
		}
		licences[name] = data
	}
	return licences, nil
}

// Boot is what an image needs to start Meridian: the signed loader, the
// certificate of the key that signed it, the kernel, the shim, and the
// licences of the fonts in the loader and the kernel.
type Boot struct {
	Loader, Certificate, Kernel []byte
	Shim                        Shim
	FontLicences                map[string][]byte
	// Programs are the programs for the kernel to start, by file name.
	Programs map[string][]byte
}

// KernelPath is where the loader looks for the kernel.
const KernelPath = "meridian/kernel"

// ProgramsPath is the folder the loader reads programs from, for the
// kernel to start.
const ProgramsPath = "meridian/programs"

// ProgramExt ends the name of every native program: an ELF file with the
// notes that make it Meridian's (docs/formats/mexec.md).
const ProgramExt = ".mexec"

// SettingsPath is where the loader reads its settings.
const SettingsPath = "meridian/boot.meri"

// Settings is the boot.meri every image starts with: the defaults, written
// out so a person can see what there is to change.
const Settings = `# Settings of the Meridian boot screen. They take effect at the next start.

boot {
    # How long the boot screen waits before it starts the default system.
    # 0s waits for a key.
    timeout = 3s
    # The system that starts when the time is up: meridian, the name on
    # another system's card, or last for the one started last time.
    default = meridian
    # true opens the boot screen with the log instead of the cards.
    verbose = false
}
`

// testSettings are the settings of test images, which the test image
// checks it read: every value differs from its default.
const testSettings = `boot {
    timeout = 2500ms
    default = example
    verbose = true
}
`

// BootFiles places the shim, the loader, the kernel, its programs, the
// settings and the licences on the EFI system partition, with what
// enrolling the loader's key needs.
func BootFiles(b Boot) map[string][]byte {
	files := map[string][]byte{
		FirmwarePath:     b.Shim.Shim,
		LoaderPath:       b.Loader,
		ManagerPath:      b.Shim.Manager,
		ShimLicencesPath: b.Shim.Licences,
		CertificatePath:  b.Certificate,
		KernelPath:       b.Kernel,
		SettingsPath:     []byte(Settings),
	}
	for name, data := range b.Programs {
		files[ProgramsPath+"/"+name] = data
	}
	for name, data := range b.FontLicences {
		files[LicencesPath+"/"+name] = data
	}
	return files
}

// ExamplePath is where test images put a second copy of the loader, for
// the boot screen to find as another system and start.
const ExamplePath = "EFI/example/grubx64.efi"

// FirmwareExamplePath is where test images put a third copy of the loader,
// under a name the partition scan does not know, for a boot option the
// test image adds to the firmware's list.
const FirmwareExamplePath = "EFI/other/tool.efi"

// LongName is a file at the root of test images whose name is as long as
// a path the loader opens can be: with its leading slash, 255 characters.
var LongName = "long-name-" + strings.Repeat("x", 244)

// AgainSuffix names the second copy test images carry of each program, so
// the loader and the kernel go through a list of more than one.
const AgainSuffix = "_again"

// TestFiles are the files of a test image: the boot files with settings
// of their own, the loader twice more as examples of other systems, each
// program a second time, and a file with a name as long as a path may be.
func TestFiles(b Boot) map[string][]byte {
	files := BootFiles(b)
	files[ExamplePath] = b.Loader
	files[FirmwareExamplePath] = b.Loader
	files[SettingsPath] = []byte(testSettings)
	files[LongName] = []byte("long name\n")
	for name, data := range b.Programs {
		files[ProgramsPath+"/"+strings.TrimSuffix(name, ProgramExt)+AgainSuffix+ProgramExt] = data
	}
	return files
}

// fatEpoch is the earliest time FAT can store. Reproducible builds often
// set SOURCE_DATE_EPOCH to 0 or 1, which FAT cannot represent.
var fatEpoch = time.Date(1980, 1, 1, 0, 0, 0, 0, time.UTC)

func fatTime(t time.Time) time.Time {
	if t.Before(fatEpoch) {
		return fatEpoch
	}
	return t
}

func volumeID(seed string) uint32 {
	return crc32.ChecksumIEEE([]byte("fat:" + seed))
}

// Disk writes a raw GPT disk image with one EFI system partition.
func Disk(path string, c Contents) error {
	f, err := os.Create(path)
	if err != nil {
		return err
	}
	err = writeDisk(f, c)
	if closeErr := f.Close(); err == nil {
		err = closeErr
	}
	return err
}

func writeDisk(f *os.File, c Contents) error {
	d := gpt.Disk{
		Size: ESPSize + 2<<20,
		Seed: c.Seed,
		Partitions: []gpt.Partition{
			{Name: "EFI system partition", Type: gpt.EFISystemPartition, Size: ESPSize},
		},
	}
	if err := f.Truncate(d.Size); err != nil {
		return err
	}
	layout, err := gpt.Write(f, d)
	if err != nil {
		return fmt.Errorf("partition table: %w", err)
	}
	esp := layout.Partitions[0]
	_, err = fat.Write(f, esp.Offset(), fat.Volume{
		Size:          ESPSize,
		Label:         "MERIDIAN",
		ID:            volumeID(c.Seed),
		HiddenSectors: uint32(esp.FirstLBA),
		Time:          fatTime(c.Time),
		Files:         c.Files,
	})
	if err != nil {
		return fmt.Errorf("EFI system partition: %w", err)
	}
	return nil
}

// ISO writes an ISO 9660 image whose El Torito boot entry is the EFI
// system partition. The files are also visible on the CD itself, all but
// those whose names ISO 9660 cannot hold, such as the test images' long
// one; the firmware and the loader only read the partition.
func ISO(path string, c Contents) error {
	esp, err := os.CreateTemp("", "sextant-esp-")
	if err != nil {
		return err
	}
	defer os.Remove(esp.Name())
	defer esp.Close()
	if err := esp.Truncate(ESPSize); err != nil {
		return err
	}
	if _, err := fat.Write(esp, 0, fat.Volume{Size: ESPSize, Label: "MERIDIAN", ID: volumeID(c.Seed), Time: fatTime(c.Time), Files: c.Files}); err != nil {
		return fmt.Errorf("EFI system partition: %w", err)
	}
	boot, err := os.ReadFile(esp.Name())
	if err != nil {
		return err
	}
	f, err := os.Create(path)
	if err != nil {
		return err
	}
	_, err = iso.Write(f, iso.Image{VolumeID: "MERIDIAN", Time: c.Time, Files: onCD(c.Files), BootImage: boot})
	if closeErr := f.Close(); err == nil {
		err = closeErr
	}
	return err
}

// onCD are the files ISO 9660 can name.
func onCD(files map[string][]byte) map[string][]byte {
	out := make(map[string][]byte, len(files))
	for p, data := range files {
		if iso.Fits(p) {
			out[p] = data
		}
	}
	return out
}

// BuildTime is the time stamped into images. SOURCE_DATE_EPOCH wins, as
// for every reproducible build; otherwise it is the time of the commit the
// checkout is at, so rebuilding a commit gives the same image.
func BuildTime(ctx context.Context, root string, getenv func(string) string) (time.Time, error) {
	if v := getenv("SOURCE_DATE_EPOCH"); v != "" {
		secs, err := strconv.ParseInt(v, 10, 64)
		if err != nil {
			return time.Time{}, fmt.Errorf("SOURCE_DATE_EPOCH must be seconds since 1970, got %q", v)
		}
		return time.Unix(secs, 0).UTC(), nil
	}
	cmd := exec.CommandContext(ctx, "git", "log", "-1", "--format=%ct")
	cmd.Dir = root
	out, err := cmd.Output()
	if err != nil {
		var exit *exec.ExitError
		if errors.As(err, &exit) {
			return time.Time{}, fmt.Errorf("read the commit time with git: %s", strings.TrimSpace(string(exit.Stderr)))
		}
		return time.Time{}, fmt.Errorf("read the commit time with git: %w", err)
	}
	secs, err := strconv.ParseInt(strings.TrimSpace(string(out)), 10, 64)
	if err != nil {
		return time.Time{}, fmt.Errorf("git gave an unexpected commit time %q", out)
	}
	return time.Unix(secs, 0).UTC(), nil
}
