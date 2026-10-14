// Package config reads sextant's settings: sextant.meri at the root of the
// repository, with sextant.local.meri layered over it for settings that only
// hold on one machine.
package config

import (
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"time"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/meri"
)

const (
	File      = "sextant.meri"
	LocalFile = "sextant.local.meri"
)

// Accelerators QEMU can use. Auto picks the best one the host offers.
var accelerators = []string{"auto", "kvm", "hvf", "whpx", "tcg"}

type Config struct {
	Root     string
	MinRatio float64
	// BuildDir is where Cargo puts its output, absolute.
	BuildDir string
	// MutateScratch is where sextant mutate keeps its copies of the
	// repository, absolute, or empty to let it pick a folder.
	MutateScratch string
	QEMU          QEMU
}

type QEMU struct {
	Memory  int64
	CPUs    int
	Accel   string
	Code    string
	Vars    string
	Timeout time.Duration
	// Stall is how long the firmware may stay silent before a test boot
	// starts the machine again.
	Stall time.Duration
}

// Defaults are the values used for settings neither file sets.
func Defaults(root string) Config {
	return Config{
		Root:     root,
		MinRatio: 1.5,
		BuildDir: filepath.Join(root, "target"),
		QEMU: QEMU{
			Memory:  512 << 20,
			CPUs:    4,
			Accel:   "auto",
			Timeout: 2 * time.Minute,
			Stall:   20 * time.Second,
		},
	}
}

// Load reads the settings of the checkout at root. sextant.meri must exist;
// sextant.local.meri is optional.
func Load(root string) (Config, error) {
	doc, err := read(filepath.Join(root, File))
	if err != nil {
		return Config{}, err
	}
	local, err := read(filepath.Join(root, LocalFile))
	switch {
	case errors.Is(err, fs.ErrNotExist):
	case err != nil:
		return Config{}, err
	default:
		doc = meri.Merge(doc, local)
	}
	return fromDocument(root, doc)
}

func read(path string) (*meri.Block, error) {
	src, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	return meri.Parse(filepath.Base(path), src)
}

// absolute resolves a path from the settings: relative ones start at the
// repository.
func absolute(root, p string) string {
	if filepath.IsAbs(p) {
		return p
	}
	return filepath.Join(root, p)
}

func section(doc *meri.Block, name string) *meri.Block {
	if blocks := doc.Blocks(name); len(blocks) > 0 {
		return blocks[len(blocks)-1]
	}
	return &meri.Block{}
}

func fromDocument(root string, doc *meri.Block) (Config, error) {
	c := Defaults(root)
	var errs []error
	keep := func(err error) {
		if err != nil {
			errs = append(errs, err)
		}
	}

	stats := section(doc, "stats")
	var err error
	c.MinRatio, err = stats.Float("min-ratio", c.MinRatio)
	keep(err)
	if c.MinRatio < 0 {
		keep(fmt.Errorf("stats.min-ratio must not be negative, got %v", c.MinRatio))
	}

	dir, err := section(doc, "build").String("dir", "")
	keep(err)
	if dir != "" {
		c.BuildDir = absolute(root, dir)
	}
	scratch, err := section(doc, "mutate").String("scratch", "")
	keep(err)
	if scratch != "" {
		c.MutateScratch = absolute(root, scratch)
	}

	q := section(doc, "qemu")
	c.QEMU.Memory, err = q.Size("memory", c.QEMU.Memory)
	keep(err)
	if c.QEMU.Memory < 128<<20 {
		keep(fmt.Errorf("qemu.memory must be at least 128MiB, got %d bytes", c.QEMU.Memory))
	}
	cpus, err := q.Int("cpus", int64(c.QEMU.CPUs))
	keep(err)
	if cpus < 1 || cpus > 64 {
		keep(fmt.Errorf("qemu.cpus must be between 1 and 64, got %d", cpus))
	}
	c.QEMU.CPUs = int(cpus)
	c.QEMU.Accel, err = q.String("accel", c.QEMU.Accel)
	keep(err)
	if !slices.Contains(accelerators, c.QEMU.Accel) {
		keep(fmt.Errorf("qemu.accel must be one of %v, got %q", accelerators, c.QEMU.Accel))
	}
	c.QEMU.Code, err = q.String("firmware", "")
	keep(err)
	c.QEMU.Vars, err = q.String("firmware-vars", "")
	keep(err)
	if (c.QEMU.Code == "") != (c.QEMU.Vars == "") {
		keep(errors.New("set qemu.firmware and qemu.firmware-vars together, or neither"))
	}
	c.QEMU.Timeout, err = q.Duration("timeout", c.QEMU.Timeout)
	keep(err)
	if c.QEMU.Timeout <= 0 {
		keep(errors.New("qemu.timeout must be longer than zero"))
	}
	c.QEMU.Stall, err = q.Duration("stall", c.QEMU.Stall)
	keep(err)
	if c.QEMU.Stall <= 0 {
		keep(errors.New("qemu.stall must be longer than zero"))
	}

	if len(errs) > 0 {
		return Config{}, errors.Join(errs...)
	}
	return c, nil
}
