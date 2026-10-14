package cli

import (
	"bytes"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/doctor"
)

func TestThePlanInstallsWhatHasAPackageAndAdvisesTheRest(t *testing.T) {
	findings := []finding{
		{name: "Go", ok: true, install: "go"},
		{name: "QEMU", required: true, install: "qemu"},
		{name: "OVMF", required: true, install: "qemu"},
		{name: "zbarimg", install: "zbar"},
		{name: "sfdisk"},
		{name: "KVM", fix: "join the kvm group", install: "never"},
	}
	commands, advice := plan(doctor.Brew, findings)
	if len(commands) != 1 || strings.Join(commands[0], " ") != "brew install qemu zbar" {
		t.Errorf("commands: %v", commands)
	}
	want := []string{"sfdisk: install it by hand; sextant knows no package for it here", "KVM: join the kvm group"}
	if strings.Join(advice, "|") != strings.Join(want, "|") {
		t.Errorf("advice: %q", advice)
	}
	if commands, advice := plan(doctor.Pacman, findings[:1]); commands != nil || advice != nil {
		t.Errorf("nothing missing: %v %v", commands, advice)
	}
	if missingRequired(findings) != 2 || missingRequired(findings[:1]) != 0 {
		t.Error("QEMU and OVMF are the required ones missing")
	}
}

func TestOnlyYesInstalls(t *testing.T) {
	for answer, want := range map[string]bool{"y\n": true, "YES\n": true, " y \n": true, "y": true, "n\n": false, "\n": false, "": false, "yess\n": false} {
		var out bytes.Buffer
		if got := confirm(strings.NewReader(answer), &out); got != want {
			t.Errorf("%q: %v", answer, got)
		}
		if out.String() != "Install it now? [y/N] \n" {
			t.Errorf("prompt: %q", out.String())
		}
	}
}

func TestInstallingRunsEachCommandUntilOneFails(t *testing.T) {
	var out, errOut bytes.Buffer
	dir := t.TempDir()
	err := install([][]string{{"go", "env", "GOOS"}, {"go", "env", "GOARCH"}}, dir, &out, &errOut)
	if err != nil || out.String() != "== go env GOOS\n"+runtime.GOOS+"\n== go env GOARCH\n"+runtime.GOARCH+"\n" {
		t.Fatalf("%v\n%s", err, out.String())
	}
	out.Reset()
	err = install([][]string{{"go", "tool", "no-such-tool"}, {"go", "env", "GOOS"}}, dir, &out, &errOut)
	if err == nil || !strings.HasPrefix(err.Error(), "go tool no-such-tool failed: ") || strings.Contains(out.String(), "GOOS") {
		t.Fatalf("%v\n%s", err, out.String())
	}
}

func TestKVMNeedsADeviceThisUserMayWrite(t *testing.T) {
	dir := t.TempDir()
	if f := kvmFinding(filepath.Join(dir, "kvm")); f.ok || !strings.Contains(f.fix, "virtualisation") {
		t.Errorf("no device: %+v", f)
	}
	writable := filepath.Join(dir, "writable")
	if err := os.WriteFile(writable, nil, 0o600); err != nil {
		t.Fatal(err)
	}
	if f := kvmFinding(writable); !f.ok || f.name != "KVM" || f.required {
		t.Errorf("a device to write: %+v", f)
	}
	readOnly := filepath.Join(dir, "read-only")
	if err := os.WriteFile(readOnly, nil, 0o400); err != nil {
		t.Fatal(err)
	}
	if f := kvmFinding(readOnly); f.ok || !strings.Contains(f.fix, "kvm group") {
		t.Errorf("a device only to read: %+v", f)
	}
}

func TestFindingsAreReportedByWhatTheyMean(t *testing.T) {
	var out bytes.Buffer
	report(&out, []finding{
		{name: "Go", what: "builds sextant", ok: true},
		{name: "QEMU", what: "boots the images", required: true},
		{name: "zbarimg", what: "reads QR codes"},
	})
	want := "ok       Go: builds sextant\nmissing  QEMU: boots the images\noptional zbarimg: reads QR codes\n"
	if out.String() != want {
		t.Errorf("got\n%s", out.String())
	}
}

func TestGoIsFoundWithItsVersion(t *testing.T) {
	path, err := exec.LookPath("go")
	if err != nil {
		t.Fatal("the tests run under go")
	}
	f := goFinding(finding{name: "Go", what: "builds sextant", ok: true}, path)
	if !f.ok || !strings.HasPrefix(f.what, "builds sextant, version 1.") {
		t.Errorf("%+v", f)
	}
	f = goFinding(finding{name: "Go", ok: true}, filepath.Join(t.TempDir(), "no-go"))
	if f.ok || f.fix != "go version printed nothing sextant can read" {
		t.Errorf("a go that prints nothing: %+v", f)
	}
}

// A machine with nothing: a PATH that holds only an empty apt-get, which
// is never run, a repository without rust-toolchain.toml, and firmware
// settings that name files that are not there.
func TestDoctorOnABareMachine(t *testing.T) {
	root := checkout(t, map[string]string{
		"sextant.local.meri":  "qemu {\n    firmware = \"/no/code.fd\"\n    firmware-vars = \"/no/vars.fd\"\n}\n",
		"rust-toolchain.toml": "[toolchain]\nchannel = \"1.99.0\"\n",
	})
	dir := t.TempDir()
	name := "apt-get"
	if runtime.GOOS == "windows" {
		name += ".exe"
	}
	if err := os.WriteFile(filepath.Join(dir, name), nil, 0o755); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", dir)
	doctorIn := func(answer string, args ...string) (string, string, int) {
		var out, errOut bytes.Buffer
		code := Run(append([]string{"doctor"}, args...), Env{Dir: root, Stdin: strings.NewReader(answer), Stdout: &out, Stderr: &errOut})
		return out.String(), errOut.String(), code
	}
	got, errs, code := doctorIn("n\n")
	for _, want := range []string{
		"missing  Go: builds sextant\n",
		"missing  rustup: installs the pinned Rust toolchain\n",
		"missing  QEMU: boots the images\n",
		"optional git: stamps images with the commit\n",
		"optional zbarimg: reads the QR codes on the screens\n",
		"missing  Rust toolchain: compiles the loader and the kernel, 1.99.0\n",
		"missing  OVMF: the UEFI firmware QEMU boots\n",
		"Rust toolchain: run 'rustup toolchain install' at the root of the repository; it reads rust-toolchain.toml\n",
		"To install what is missing:\n  sudo apt-get install -y golang-go rustup qemu-system-x86 git openssl xorriso mtools dosfstools fdisk libarchive-tools qrencode zbar-tools ovmf\n",
		"Install it now? [y/N] \n",
	} {
		if !strings.Contains(got, want) {
			t.Errorf("%q missing from\n%s", want, got)
		}
	}
	if code != ExitFailure || errs != "sextant doctor: 5 things Meridian needs are missing\n" || strings.Contains(got, "== ") {
		t.Errorf("exit %d, %q: nothing should be installed on a no", code, errs)
	}
	// Without a package manager there is nothing to run, only advice.
	if err := os.Remove(filepath.Join(dir, name)); err != nil {
		t.Fatal(err)
	}
	got, errs, code = doctorIn("y\n")
	if code != ExitFailure || !strings.Contains(got, "QEMU: install it by hand; sextant knows no package for it here\n") ||
		strings.Contains(got, "To install") || strings.Contains(got, "[y/N]") || strings.Contains(got, "Everything") ||
		errs != "sextant doctor: 5 things Meridian needs are missing\n" {
		t.Errorf("no manager: exit %d\n%s%s", code, got, errs)
	}
	if err := os.WriteFile(filepath.Join(dir, name), nil, 0o755); err != nil {
		t.Fatal(err)
	}
	// Installing needs sudo, which this PATH has not, so it fails before
	// it could change anything.
	for _, way := range [][]string{{"y\n"}, {"", "--yes"}} {
		got, errs, code := doctorIn(way[0], way[1:]...)
		if code != ExitFailure || !strings.Contains(got, "== sudo apt-get install -y golang-go") ||
			!strings.HasPrefix(errs, "sextant doctor: sudo apt-get install -y golang-go rustup qemu-system-x86 git") {
			t.Errorf("%v: exit %d\n%s%s", way, code, got, errs)
		}
	}
}

func TestTheToolchainFileMustNameAChannel(t *testing.T) {
	for name, files := range map[string]map[string]string{
		"none":       {},
		"no channel": {"rust-toolchain.toml": "[toolchain]\nprofile = \"minimal\"\n"},
	} {
		s := &session{root: checkout(t, files)}
		if f := s.toolchainFinding(); f.ok || !f.required || f.fix != "rust-toolchain.toml is missing or names no channel" {
			t.Errorf("%s: %+v", name, f)
		}
	}
}

// Only what the tests use to check sextant's work is missing: the
// machine can build and boot, so doctor advises and succeeds. The PATH
// holds links to the real Go, rustup and QEMU, so this runs where those
// are and links can be made.
func TestDoctorSucceedsWithOnlyOptionalToolsMissing(t *testing.T) {
	if runtime.GOOS != "linux" {
		t.Skip("the firmware and KVM it needs are found this way on Linux")
	}
	dir := t.TempDir()
	for _, tool := range []string{"go", "rustup", "qemu-system-x86_64"} {
		real, err := exec.LookPath(tool)
		if err != nil {
			t.Skipf("%s is not installed", tool)
		}
		if err := os.Symlink(real, filepath.Join(dir, tool)); err != nil {
			t.Skip(err)
		}
	}
	root := realRoot(t)
	t.Setenv("PATH", dir)
	var out, errOut bytes.Buffer
	code := Run([]string{"doctor"}, Env{Dir: root, Stdin: strings.NewReader("n\n"), Stdout: &out, Stderr: &errOut})
	got := out.String()
	if code != ExitOK || errOut.Len() != 0 || strings.Contains(got, "missing  ") ||
		!strings.Contains(got, "optional zbarimg: ") || !strings.Contains(got, "zbarimg: install it by hand") ||
		strings.Contains(got, "Everything") || !strings.Contains(got, "KVM: ") {
		t.Errorf("exit %d\n%s%s", code, got, errOut.String())
	}
}

// The whole command on this machine, answering no: whatever it finds, Go
// is there, and it fails exactly when something it needs is missing.
func TestDoctorSaysWhatThisMachineHas(t *testing.T) {
	var out, errOut bytes.Buffer
	code := Run([]string{"doctor"}, Env{Dir: realRoot(t), Stdin: strings.NewReader("n\n"), Stdout: &out, Stderr: &errOut})
	got := out.String()
	if !strings.Contains(got, "ok       Go: builds sextant, version 1.") || !strings.Contains(got, "Rust toolchain: compiles the loader and the kernel, ") {
		t.Fatalf("got\n%s%s", got, errOut.String())
	}
	missing := 0
	for _, line := range strings.Split(got, "\n") {
		if strings.HasPrefix(line, "missing  ") {
			missing++
		}
	}
	failed := code != ExitOK
	if failed != (missing > 0) || failed != strings.Contains(errOut.String(), "Meridian needs") {
		t.Errorf("%d missing, exit %d\n%s%s", missing, code, got, errOut.String())
	}
	if code == ExitOK && !strings.Contains(got, "Everything Meridian needs is here.") && !strings.Contains(got, "optional ") {
		t.Errorf("all there, yet it did not say so:\n%s", got)
	}
	if _, err := exec.LookPath("git"); err == nil && !strings.Contains(got, "ok       git: stamps images with the commit\n") {
		t.Errorf("only QEMU's line carries a version besides Go's:\n%s", got)
	}
	if _, err := exec.LookPath("qemu-system-x86_64"); err == nil && !strings.Contains(got, "ok       QEMU: boots the images, version ") {
		t.Errorf("QEMU's version:\n%s", got)
	}
	if strings.Contains(got, "KVM: makes QEMU fast") != (runtime.GOOS == "linux") {
		t.Errorf("KVM is looked for on Linux only:\n%s", got)
	}
	if r := run("doctor", "extra"); r.code != ExitUsage {
		t.Errorf("an argument: %d", r.code)
	}
}
