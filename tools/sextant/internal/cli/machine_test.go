package cli

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/cargo"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/config"
)

func needTool(t *testing.T, name string) {
	t.Helper()
	if _, err := exec.LookPath(name); err != nil {
		if os.Getenv("SEXTANT_REQUIRE_TOOLS") == "1" {
			t.Fatalf("%s is required but missing", name)
		}
		t.Skipf("%s is missing", name)
	}
}

func realRoot(t *testing.T) string {
	t.Helper()
	root, err := filepath.Abs("../../../..")
	if err != nil {
		t.Fatal(err)
	}
	return root
}

func TestCommandsThatTakeNoArguments(t *testing.T) {
	root := checkout(t, copyRepo(smallRepo))
	for _, cmd := range []string{"build", "image", "run", "test"} {
		r := runIn(root, cmd, "extra")
		want := "sextant " + cmd + ": " + cmd + " takes no arguments, got \"extra\"\n"
		if r.code != ExitUsage || r.stderr != want {
			t.Errorf("%s: exit %d, stderr %q", cmd, r.code, r.stderr)
		}
		r = runIn(root, cmd, "--no-such-flag")
		if r.code != ExitUsage || !strings.Contains(r.stderr, "flag provided but not defined") {
			t.Errorf("%s: exit %d, stderr %q", cmd, r.code, r.stderr)
		}
	}
}

func TestBuildMakesNoISO(t *testing.T) {
	r := runIn(checkout(t, copyRepo(smallRepo)), "build", "--iso")
	if r.code != ExitUsage || !strings.Contains(r.stderr, "use 'sextant image --iso' for an ISO") {
		t.Fatalf("exit %d, stderr %q", r.code, r.stderr)
	}
}

func TestCommandsNeedACheckout(t *testing.T) {
	for _, cmd := range []string{"build", "image", "run", "test"} {
		r := runIn(t.TempDir(), cmd)
		if r.code != ExitFailure || !strings.Contains(r.stderr, "not inside a Meridian checkout") {
			t.Errorf("%s: exit %d, stderr %q", cmd, r.code, r.stderr)
		}
	}
}

func TestBrokenSettingsStopEveryCommand(t *testing.T) {
	root := checkout(t, copyRepo(smallRepo))
	if err := os.WriteFile(filepath.Join(root, "sextant.meri"), []byte("qemu { cpus = 0 }\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	for _, cmd := range []string{"build", "image", "run", "test"} {
		r := runIn(root, cmd)
		if r.code != ExitFailure || !strings.Contains(r.stderr, "qemu.cpus must be between 1 and 64") {
			t.Errorf("%s: exit %d, stderr %q", cmd, r.code, r.stderr)
		}
	}
}

func testSession(buildDir string) *session {
	s := &session{root: "/repo", settings: config.Defaults("/repo")}
	s.settings.BuildDir = buildDir
	return s
}

func TestImagePaths(t *testing.T) {
	s := testSession("/b")
	tests := []struct {
		v    variant
		want string
	}{
		{variant{}, filepath.Join("/b", "meridian.img")},
		{variant{iso: true}, filepath.Join("/b", "meridian.iso")},
		{variant{testMode: true}, filepath.Join("/b", "meridian-test.img")},
		{variant{testMode: true, iso: true}, filepath.Join("/b", "meridian-test.iso")},
		{variant{release: true}, filepath.Join("/b", "meridian.img")},
	}
	for _, tt := range tests {
		if got := s.imagePath(tt.v); got != tt.want {
			t.Errorf("%+v: got %s, want %s", tt.v, got, tt.want)
		}
	}
}

func TestBuildsFollowTheVariant(t *testing.T) {
	s := testSession("/b")
	for _, part := range []cargo.Part{cargo.Loader, cargo.Kernel} {
		b := s.build(part, variant{release: true, testMode: true})
		if b.Part != part || b.Root != "/repo" || b.BuildDir != "/b" || !b.Release || !b.QEMUTest {
			t.Fatalf("got %+v", b)
		}
		if b := s.build(part, variant{}); b.Release || b.QEMUTest {
			t.Fatalf("a plain variant built %+v", b)
		}
	}
}

func TestIdentityOutsideGit(t *testing.T) {
	s := testSession(t.TempDir())
	s.root = t.TempDir()
	t.Setenv("SOURCE_DATE_EPOCH", "")
	seed, when := s.identity(context.Background())
	if seed != "meridian" || !when.Equal(time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC)) {
		t.Fatalf("got %q, %v", seed, when)
	}
}

func TestIdentityFollowsTheCommit(t *testing.T) {
	needTool(t, "git")
	s := testSession(t.TempDir())
	s.root = realRoot(t)
	t.Setenv("SOURCE_DATE_EPOCH", "1760418000")
	seed, when := s.identity(context.Background())
	head, err := exec.Command("git", "-C", s.root, "rev-parse", "HEAD").Output()
	if err != nil {
		t.Fatal(err)
	}
	if seed != "meridian "+strings.TrimSpace(string(head)) || !when.Equal(time.Unix(1760418000, 0)) {
		t.Fatalf("got %q, %v", seed, when)
	}
}

func TestMachineFromTheSettings(t *testing.T) {
	needTool(t, "qemu-system-x86_64")
	s := testSession(t.TempDir())
	s.settings.QEMU.Memory = 1 << 30
	s.settings.QEMU.CPUs = 3
	s.settings.QEMU.Accel = "tcg"
	m, err := s.machine(variant{testMode: true}, "/x.img")
	if err != nil {
		if os.Getenv("SEXTANT_REQUIRE_TOOLS") == "1" {
			t.Fatal(err)
		}
		t.Skip(err)
	}
	if m.Memory != 1<<30 || m.CPUs != 3 || strings.Join(m.Accels, ",") != "tcg" || !m.DebugExit || m.Disk != "/x.img" || m.CDROM != "" {
		t.Fatalf("got %+v", m)
	}
	m, err = s.machine(variant{iso: true}, "/x.iso")
	if err != nil || m.CDROM != "/x.iso" || m.Disk != "" || m.DebugExit {
		t.Fatalf("got %+v, %v", m, err)
	}
	s.settings.QEMU.Code = "/missing/code.fd"
	s.settings.QEMU.Vars = "/missing/vars.fd"
	if _, err := s.machine(variant{}, "/x.img"); err == nil || !strings.Contains(err.Error(), "/missing/code.fd") {
		t.Fatalf("a configured firmware that is missing must be reported, got %v", err)
	}
}

func TestMachineWithoutQEMU(t *testing.T) {
	t.Setenv("PATH", t.TempDir())
	_, err := testSession(t.TempDir()).machine(variant{}, "/x.img")
	if err == nil || err.Error() != "qemu-system-x86_64 is not installed or not on the PATH; run 'sextant doctor' to install it" {
		t.Fatalf("got %v", err)
	}
}

func TestImageAndBuildOnTheRealRepository(t *testing.T) {
	needTool(t, "cargo")
	out := filepath.Join(t.TempDir(), "disk.img")
	r := runIn(realRoot(t), "image", "--out", out)
	if r.code != ExitOK || r.stdout != out+"\n" {
		t.Fatalf("exit %d, stdout %q, stderr %s", r.code, r.stdout, r.stderr)
	}
	if info, err := os.Stat(out); err != nil || info.Size() < 64<<20 {
		t.Fatalf("image: %v, %v", info, err)
	}
	r = runIn(realRoot(t), "build")
	lines := strings.Split(strings.TrimSuffix(r.stdout, "\n"), "\n")
	if r.code != ExitOK || len(lines) != 2 ||
		!strings.HasSuffix(lines[0], filepath.Join("x86_64-unknown-uefi", "debug", "meridian.efi")) ||
		!strings.HasSuffix(lines[1], filepath.Join("x86_64-unknown-none", "debug", "kernel")) {
		t.Fatalf("exit %d, stdout %q", r.code, r.stdout)
	}
}

func TestQEMUTestsOnTheRealRepository(t *testing.T) {
	needTool(t, "cargo")
	needTool(t, "qemu-system-x86_64")
	r := runIn(realRoot(t), "test", "--qemu")
	if r.code != ExitOK {
		t.Fatalf("exit %d\nstdout:\n%s\nstderr:\n%s", r.code, r.stdout, r.stderr)
	}
	for _, want := range []string{"== QEMU test image\n", "ok      uefi.keyboard.enter\n", "ok      boot.uefi_version\n", "ok      boot.kernel.file\n", "checks passed in QEMU\n"} {
		if !strings.Contains(r.stdout, want) {
			t.Errorf("missing %q in:\n%s", want, r.stdout)
		}
	}
}

// qemuSession runs the QEMU tests of the real repository with settings the
// test chooses, building into a cache shared with the other boot tests.
func qemuSession(t *testing.T) (*session, *strings.Builder) {
	t.Helper()
	needTool(t, "cargo")
	needTool(t, "qemu-system-x86_64")
	var out strings.Builder
	s := testSession(filepath.Join(os.TempDir(), "sextant-test-build"))
	s.root = realRoot(t)
	s.env = Env{Stdout: &out, Stderr: &out, ConfigDir: testConfigDir}
	return s, &out
}

func TestQEMUTestsReportAFirmwareThatNeverPrints(t *testing.T) {
	s, out := qemuSession(t)
	s.settings.QEMU.Stall = time.Millisecond
	err := s.qemuTests(context.Background())
	if err == nil || !strings.HasPrefix(err.Error(), "the firmware printed nothing in 3 attempts; check that ") {
		t.Fatalf("got %v", err)
	}
	if strings.Count(out.String(), "starting the machine again") != 2 {
		t.Fatalf("two restarts should be visible:\n%s", out.String())
	}
}

func TestQEMUTestsReportARunThatNeverFinishes(t *testing.T) {
	s, _ := qemuSession(t)
	// Long before OVMF has even loaded the loader; a whole test boot with
	// KVM can take less than a second.
	s.settings.QEMU.Timeout = 200 * time.Millisecond
	err := s.qemuTests(context.Background())
	if err == nil || !strings.Contains(err.Error(), "the image never reported a test before the timeout") {
		t.Fatalf("got %v", err)
	}
}
