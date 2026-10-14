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

// LoaderPath is where UEFI firmware looks for a loader on removable media
// and on disks without boot entries.
const LoaderPath = "EFI/BOOT/BOOTX64.EFI"

// KernelPath is where the loader looks for the kernel.
const KernelPath = "meridian/kernel"

// BootFiles places the loader and the kernel on the EFI system partition.
func BootFiles(loader, kernel []byte) map[string][]byte {
	return map[string][]byte{LoaderPath: loader, KernelPath: kernel}
}

// ExamplePath is where test images put a second copy of the loader, for
// the boot screen to find as another system and start.
const ExamplePath = "EFI/example/grubx64.efi"

// TestFiles are the files of a test image: the boot files, and the
// loader once more as an example of another system.
func TestFiles(loader, kernel []byte) map[string][]byte {
	files := BootFiles(loader, kernel)
	files[ExamplePath] = loader
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
// system partition. The files are also visible on the CD itself.
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
	_, err = iso.Write(f, iso.Image{VolumeID: "MERIDIAN", Time: c.Time, Files: c.Files, BootImage: boot})
	if closeErr := f.Close(); err == nil {
		err = closeErr
	}
	return err
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
