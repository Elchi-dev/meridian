package cli

import (
	"bytes"
	"context"
	"errors"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/cargo"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/config"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/image"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/qemu"
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
	// A repository of its own: mutation testing runs this in copies that
	// leave the real history out.
	s.root = checkout(t, map[string]string{"f": "x"})
	git(t, s.root, "init", "-q")
	git(t, s.root, "add", ".")
	git(t, s.root, "commit", "-q", "-m", "start")
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
	disk, err := os.ReadFile(out)
	if err != nil || !bytes.Contains(disk, []byte(image.Settings)) || bytes.Contains(disk, []byte("default = example")) {
		t.Errorf("a plain image should carry the default settings, not the test image's: %v", err)
	}
	r = runIn(realRoot(t), "build")
	lines := strings.Split(strings.TrimSuffix(r.stdout, "\n"), "\n")
	if r.code != ExitOK || len(lines) != 5 ||
		!strings.HasSuffix(lines[0], filepath.Join("x86_64-unknown-uefi", "debug", "meridian.efi")) ||
		!strings.HasSuffix(lines[1], filepath.Join("x86_64-unknown-none", "debug", "kernel")) ||
		!strings.HasSuffix(lines[2], filepath.Join("x86_64-unknown-none", "debug", "examples", "hello")) ||
		!strings.HasSuffix(lines[3], filepath.Join("x86_64-unknown-none", "debug", "examples", "init")) ||
		!strings.HasSuffix(lines[4], filepath.Join("x86_64-unknown-none", "debug", "examples", "shell")) {
		t.Fatalf("exit %d, stdout %q", r.code, r.stdout)
	}
	if loader, err := os.ReadFile(lines[0]); err != nil || bytes.Contains(loader, []byte("TESTS DONE")) {
		t.Errorf("build should make the loader without the test image's checks: %v", err)
	}
}

func TestOnlyTestImagesCarryTheTestPrograms(t *testing.T) {
	needTool(t, "cargo")
	s := testSession(filepath.Join(os.TempDir(), "sextant-test-build"))
	s.root = realRoot(t)
	s.env = Env{Stdout: io.Discard, Stderr: io.Discard}
	for _, v := range []variant{{}, {testMode: true}} {
		b, err := s.buildBoot(t.Context(), v)
		if err != nil {
			t.Fatal(err)
		}
		want := len(cargo.Programs)
		if v.testMode {
			want += len(cargo.TestPrograms)
		}
		_, hello := b.programs["hello.mexec"]
		_, exitWith := b.programs["exitwith.mexec"]
		_, whoAmI := b.programs["whoami.mexec"]
		_, threads := b.programs["threads.mexec"]
		_, shell := b.programs["shell.mexec"]
		_, init := b.programs["init.mexec"]
		if !hello || !shell || !init || exitWith != v.testMode || whoAmI != v.testMode || threads != v.testMode || len(b.programs) != want {
			t.Errorf("test mode %v: %v", v.testMode, b.programs)
		}
	}
}

func TestTheTestImageBootsAgainOnAnOlderMachine(t *testing.T) {
	m := qemu.Machine{Binary: "qemu-system-x86_64", Memory: 512, CPUs: 4, Accels: []string{"kvm", "tcg"}, Disk: "/build/test.img", DebugExit: true}
	passes := testPasses(m, "/build/screens")
	if len(passes) != 2 {
		t.Fatalf("%d passes, want 2", len(passes))
	}
	first, second := passes[0], passes[1]
	if first.title != "== QEMU test image" || !reflect.DeepEqual(first.machine, m) || first.screens != "/build/screens" {
		t.Errorf("first pass: %+v", first)
	}
	if second.title != "== QEMU test image without x2APIC, XSAVE or PS/2" || !second.machine.WithoutX2APIC || !second.machine.WithoutXSAVE || !second.machine.WithoutPS2 || second.screens != "" {
		t.Errorf("the second pass should hide the x2APIC, XSAVE and PS/2 and keep no screens: %+v", second)
	}
	second.machine.WithoutX2APIC = false
	second.machine.WithoutXSAVE = false
	second.machine.WithoutPS2 = false
	if !reflect.DeepEqual(second.machine, m) {
		t.Errorf("the second machine should differ in nothing else: %+v", second.machine)
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

func TestDevelopmentKeysAreNamedAfterTheMachine(t *testing.T) {
	for _, c := range []struct {
		host string
		err  error
		want string
	}{
		{"workshop", nil, "Meridian development key, workshop"},
		{"", nil, "Meridian development key, an unnamed machine"},
		{"workshop", errors.New("no name to be had"), "Meridian development key, an unnamed machine"},
	} {
		if got := developmentKeyName(c.host, c.err); got != c.want {
			t.Errorf("%q, %v: %q, want %q", c.host, c.err, got, c.want)
		}
	}
}

func TestRunSettingsFromTheCommandLine(t *testing.T) {
	r, err := parseRun(nil)
	if err != nil || !r.display || r.gdb || r.limit != 24*time.Hour || r.until != "" || r.v != (variant{}) {
		t.Fatalf("defaults: %+v, %v", r, err)
	}
	r, err = parseRun([]string{"--headless", "--debug", "--for", "30s", "--until", "Meridian kernel", "--release"})
	if err != nil || r.display || !r.gdb || r.limit != 30*time.Second || r.until != "Meridian kernel" || !r.v.release {
		t.Fatalf("all set: %+v, %v", r, err)
	}
	for _, bad := range []string{"0s", "-1s"} {
		var usage *UsageError
		if _, err := parseRun([]string{"--for", bad}); !errors.As(err, &usage) || !strings.Contains(err.Error(), "--for takes a time") {
			t.Errorf("--for %s: %v", bad, err)
		}
	}
}

func TestHowARunEnds(t *testing.T) {
	broken := errors.New("QEMU would not start")
	waiting := runSettings{until: "Meridian kernel"}
	for _, c := range []struct {
		name string
		r    runSettings
		res  qemu.Result
		err  error
		want string
	}{
		{"QEMU failed", runSettings{}, qemu.Result{}, broken, "QEMU would not start"},
		{"closed", runSettings{}, qemu.Result{}, nil, ""},
		{"crashed", runSettings{}, qemu.Result{ExitCode: 1}, nil, "QEMU exited with 1"},
		{"time over", runSettings{}, qemu.Result{TimedOut: true, ExitCode: -1}, nil, ""},
		{"text seen", waiting, qemu.Result{StepsDone: 1, Stopped: true, ExitCode: -1}, nil, ""},
		{"text never seen", waiting, qemu.Result{TimedOut: true, ExitCode: -1}, nil, `"Meridian kernel" did not appear on the serial port`},
		{"ended before the text", waiting, qemu.Result{}, nil, `"Meridian kernel" did not appear on the serial port`},
	} {
		err := runOutcome(c.r, c.res, c.err)
		if (c.want == "") != (err == nil) || (err != nil && err.Error() != c.want) {
			t.Errorf("%s: %v, want %q", c.name, err, c.want)
		}
	}
}

func TestWhichTestsRun(t *testing.T) {
	for _, c := range []struct {
		args         []string
		host, inQEMU bool
	}{
		{nil, true, true},
		{[]string{"--host"}, true, false},
		{[]string{"--qemu"}, false, true},
		{[]string{"--host", "--qemu"}, true, true},
	} {
		scope, err := parseTest(c.args)
		if err != nil || scope != (testScope{c.host, c.inQEMU}) {
			t.Errorf("%q: %+v, %v", c.args, scope, err)
		}
	}
}

func TestTheHostTestsAreCargosThenGos(t *testing.T) {
	s := testSession(filepath.Join("b", "uild"))
	s.root = filepath.Join("r", "oot")
	want := []step{
		{s.root, "cargo", []string{"test", "--workspace", "--locked"}, []string{"CARGO_TARGET_DIR=" + filepath.Join("b", "uild"), "SEXTANT_INSIDE_CHECKS=1"}},
		{filepath.Join(s.root, "tools", "sextant"), "go", []string{"test", "./..."}, []string{"SEXTANT_INSIDE_CHECKS=1"}},
	}
	if got := s.hostSteps(); !reflect.DeepEqual(got, want) {
		t.Fatalf("got %+v", got)
	}
}

func TestStepsRunInTheirFolderAndStopAtAFailure(t *testing.T) {
	needTool(t, "go")
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "go.mod"), []byte("module stepcheck\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	var out, errOut strings.Builder
	err := runSteps(t.Context(), []step{
		{dir, "go", []string{"list", "-m"}, nil},
		{dir, "go", []string{"env", "GOOS"}, []string{"GOOS=plan9"}},
	}, &out, &errOut)
	if err != nil || out.String() != "== go list -m\nstepcheck\n== go env GOOS\nplan9\n" {
		t.Fatalf("%v\nstdout %q\nstderr %q", err, out.String(), errOut.String())
	}
	out.Reset()
	err = runSteps(t.Context(), []step{
		{dir, "go", []string{"no-such-command"}, nil},
		{dir, "go", []string{"env", "GOOS"}, nil},
	}, &out, &errOut)
	if err == nil || !strings.Contains(err.Error(), "go no-such-command failed") || out.String() != "== go no-such-command\n" {
		t.Fatalf("%v\nstdout %q", err, out.String())
	}
}

func TestTestStopsWhenTheHostTestsFail(t *testing.T) {
	dir := t.TempDir()
	// An empty folder holds no tests that could start these again.
	t.Setenv(insideChecks, "")
	if err := os.WriteFile(filepath.Join(dir, "sextant.meri"), nil, 0o644); err != nil {
		t.Fatal(err)
	}
	for _, args := range [][]string{{"test", "--host"}, {"test"}} {
		r := runIn(dir, args...)
		if r.code != ExitFailure || !strings.Contains(r.stderr, "cargo test --workspace --locked failed") ||
			strings.Contains(r.stdout, "== go") || strings.Contains(r.stdout, "QEMU") {
			t.Errorf("%q: exit %d\nstdout %s\nstderr %s", args, r.code, r.stdout, r.stderr)
		}
	}
}

func TestHostTestsDoNotStartInsideThemselves(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "sextant.meri"), nil, 0o644); err != nil {
		t.Fatal(err)
	}
	t.Setenv(insideChecks, "1")
	for _, args := range [][]string{{"test", "--host"}, {"test"}} {
		r := runIn(dir, args...)
		if r.code != ExitFailure || !strings.Contains(r.stderr, "inside its own host tests") || strings.Contains(r.stdout, "== cargo") {
			t.Errorf("%q: exit %d\nstdout %s\nstderr %s", args, r.code, r.stdout, r.stderr)
		}
	}
	if r := runIn(dir, "test", "--qemu"); strings.Contains(r.stderr, "inside its own host tests") {
		t.Errorf("only the host tests are held back: %s", r.stderr)
	}
}

func TestRunBootsTheRealImageUntilTheKernel(t *testing.T) {
	needTool(t, "cargo")
	needTool(t, "qemu-system-x86_64")
	r := runIn(realRoot(t), "run", "--headless", "--until", "Meridian kernel", "--for", "3m")
	if r.code != ExitOK || !strings.Contains(r.stdout, "Meridian boot") || !strings.Contains(r.stdout, "Meridian kernel") {
		t.Fatalf("exit %d\nstdout:\n%s\nstderr:\n%s", r.code, r.stdout, r.stderr)
	}
}
