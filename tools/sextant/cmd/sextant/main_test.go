package main

import (
	"errors"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func buildSextant(t *testing.T, ldflags string) string {
	t.Helper()
	bin := filepath.Join(t.TempDir(), "sextant")
	if runtime.GOOS == "windows" {
		bin += ".exe"
	}
	cmd := exec.Command("go", "build", "-ldflags", ldflags, "-o", bin, ".")
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("go build failed: %v\n%s", err, out)
	}
	return bin
}

// Release builds set the version through -ldflags. If the variable is renamed
// or moved, the linker silently ignores the flag and every release would call
// itself "dev".
func TestReleaseVersionReachesTheBinary(t *testing.T) {
	bin := buildSextant(t, "-X main.version=v1.2.3")
	out, err := exec.Command(bin, "version").Output()
	if err != nil {
		t.Fatalf("sextant version failed: %v", err)
	}
	if !strings.HasPrefix(string(out), "sextant v1.2.3") {
		t.Fatalf("output = %q, want it to start with %q", out, "sextant v1.2.3")
	}
}

func TestBinaryExitsWithTheCommandsCode(t *testing.T) {
	bin := buildSextant(t, "")
	err := exec.Command(bin, "no-such-command").Run()
	var exit *exec.ExitError
	if !errors.As(err, &exit) {
		t.Fatalf("expected a non-zero exit, got %v", err)
	}
	if exit.ExitCode() != 2 {
		t.Fatalf("exit code = %d, want 2 for a usage error", exit.ExitCode())
	}
}
