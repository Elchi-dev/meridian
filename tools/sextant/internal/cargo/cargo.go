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

const uefiTarget = "x86_64-unknown-uefi"

// Loader is a build of the UEFI bootloader.
type Loader struct {
	Root string
	// BuildDir is sextant's build directory. Test images get a folder of
	// their own inside it, so switching between running and testing does
	// not rebuild everything each time.
	BuildDir string
	Release  bool
	QEMUTest bool
}

func (l Loader) targetDir() string {
	if l.QEMUTest {
		return filepath.Join(l.BuildDir, "qemu-test")
	}
	return l.BuildDir
}

// Args is the Cargo command line, without "cargo".
func (l Loader) Args() []string {
	args := []string{"build", "--locked", "-p", "boot", "--target", uefiTarget}
	if l.Release {
		args = append(args, "--release")
	}
	if l.QEMUTest {
		args = append(args, "--features", "qemu-test")
	}
	return args
}

// Env holds the variables Cargo needs on top of the caller's environment.
func (l Loader) Env() []string {
	return []string{"CARGO_TARGET_DIR=" + l.targetDir()}
}

// Output is where Cargo writes the loader.
func (l Loader) Output() string {
	profile := "debug"
	if l.Release {
		profile = "release"
	}
	return filepath.Join(l.targetDir(), uefiTarget, profile, "meridian.efi")
}

// Build runs Cargo and returns the path of the loader it built.
func (l Loader) Build(ctx context.Context, stdout, stderr io.Writer) (string, error) {
	cmd := exec.CommandContext(ctx, "cargo", l.Args()...)
	cmd.Dir = l.Root
	cmd.Env = append(os.Environ(), l.Env()...)
	cmd.Stdout, cmd.Stderr = stdout, stderr
	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("cargo %v: %w", l.Args(), err)
	}
	out := l.Output()
	if _, err := os.Stat(out); err != nil {
		return "", fmt.Errorf("cargo finished but %s is missing: %w", out, err)
	}
	return out, nil
}
