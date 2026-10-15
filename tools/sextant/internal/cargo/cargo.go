// Package cargo runs Cargo to build Meridian's parts for their targets.
package cargo

import (
	"context"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
)

// Part is one program Cargo builds for Meridian: its package, the target it
// runs on, and the name of the file Cargo writes.
type Part struct {
	Package string
	Target  string
	File    string
	// Example is the package's example Cargo builds, when the part is one.
	Example string
}

var (
	// Loader is the UEFI bootloader.
	Loader = Part{Package: "boot", Target: "x86_64-unknown-uefi", File: "meridian.efi"}
	// Kernel is the kernel, an ELF file the loader reads.
	Kernel = Part{Package: "kernel", Target: "x86_64-unknown-none", File: "kernel"}
	// Hello is the first program, written on the program runtime.
	Hello = Part{Package: "program", Target: "x86_64-unknown-none", File: "hello", Example: "hello"}
	// ExitWith exits with the number it is sent, for the test image's
	// checks of processes.
	ExitWith = Part{Package: "program", Target: "x86_64-unknown-none", File: "exitwith", Example: "exitwith"}
	// Programs are what every image carries for the kernel to start.
	Programs = []Part{Hello}
	// TestPrograms are what test images carry on top.
	TestPrograms = []Part{ExitWith}
)

// Build is one build of a part.
type Build struct {
	Part
	Root string
	// BuildDir is sextant's build directory. Test images get a folder of
	// their own inside it, so switching between running and testing does
	// not rebuild everything each time.
	BuildDir string
	Release  bool
	QEMUTest bool
}

func (b Build) targetDir() string {
	if b.QEMUTest {
		return filepath.Join(b.BuildDir, "qemu-test")
	}
	return b.BuildDir
}

// Args is the Cargo command line, without "cargo".
func (b Build) Args() []string {
	args := []string{"build", "--locked", "-p", b.Package, "--target", b.Target}
	if b.Example != "" {
		args = append(args, "--example", b.Example)
	}
	if b.Release {
		args = append(args, "--release")
	}
	if b.QEMUTest {
		args = append(args, "--features", "qemu-test")
	}
	return args
}

// Env holds the variables Cargo needs on top of the caller's environment.
func (b Build) Env() []string {
	return []string{"CARGO_TARGET_DIR=" + b.targetDir()}
}

// Output is where Cargo writes the part.
func (b Build) Output() string {
	profile := "debug"
	if b.Release {
		profile = "release"
	}
	dir := filepath.Join(b.targetDir(), b.Target, profile)
	if b.Example != "" {
		dir = filepath.Join(dir, "examples")
	}
	return filepath.Join(dir, b.File)
}

// Run runs Cargo and returns the path of the file it built.
func (b Build) Run(ctx context.Context, stdout, stderr io.Writer) (string, error) {
	cmd := exec.CommandContext(ctx, "cargo", b.Args()...)
	cmd.Dir = b.Root
	cmd.Env = append(os.Environ(), b.Env()...)
	cmd.Stdout, cmd.Stderr = stdout, stderr
	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("cargo %v: %w", b.Args(), err)
	}
	out := b.Output()
	if _, err := os.Stat(out); err != nil {
		return "", fmt.Errorf("cargo finished but %s is missing: %w", out, err)
	}
	return out, nil
}
