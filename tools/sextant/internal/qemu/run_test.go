package qemu

import (
	"context"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/cargo"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/image"
)

// consoleProbe is CONSOLE_PROBE from lib/uefi/src/qemu_test.rs. It is
// longer than one piece of the UCS-2 encoder, so it only arrives whole when
// splitting works.
var consoleProbe = "console probe: " + strings.Repeat("0123456789abcdefghijklmnopqrstuvwxyz", 4) + " end"

// need skips the test when a tool is missing, unless SEXTANT_REQUIRE_TOOLS
// is set, as in CI, where a missing tool must fail instead of hiding.
func need(t *testing.T, found bool, what string) {
	t.Helper()
	if found {
		return
	}
	if os.Getenv("SEXTANT_REQUIRE_TOOLS") == "1" {
		t.Fatalf("%s is required but missing", what)
	}
	t.Skipf("%s is missing", what)
}

type rig struct {
	binary   string
	firmware Firmware
}

func newRig(t *testing.T) rig {
	t.Helper()
	bin, err := exec.LookPath("qemu-system-x86_64")
	need(t, err == nil, "qemu-system-x86_64")
	fw, err := FindFirmware(runtime.GOOS, bin, Firmware{}, FileExists)
	need(t, err == nil, "OVMF firmware")
	_, err = exec.LookPath("cargo")
	need(t, err == nil, "cargo")
	return rig{binary: bin, firmware: fw}
}

func (r rig) machine(disk string) Machine {
	return Machine{
		Binary:    r.binary,
		Firmware:  r.firmware,
		Memory:    256 << 20,
		CPUs:      2,
		Accels:    Accelerators("auto", runtime.GOOS, runtime.GOARCH),
		Disk:      disk,
		DebugExit: true,
	}
}

func repoRoot(t *testing.T) string {
	t.Helper()
	root, err := filepath.Abs("../../../..")
	if err != nil {
		t.Fatal(err)
	}
	return root
}

// buildLoader builds the real loader with Cargo and returns its bytes.
func buildLoader(t *testing.T, qemuTest bool) []byte {
	t.Helper()
	l := cargo.Loader{Root: repoRoot(t), BuildDir: filepath.Join(os.TempDir(), "sextant-test-build"), QEMUTest: qemuTest}
	efi, err := l.Build(context.Background(), io.Discard, io.Discard)
	if err != nil {
		t.Fatalf("build the loader: %v", err)
	}
	data, err := os.ReadFile(efi)
	if err != nil {
		t.Fatal(err)
	}
	return data
}

// diskWithLoader writes the real loader onto a disk image.
func diskWithLoader(t *testing.T, qemuTest bool) string {
	t.Helper()
	disk := filepath.Join(t.TempDir(), "meridian.img")
	contents := image.Contents{Seed: t.Name(), Time: time.Unix(1_760_000_000, 0), Files: image.LoaderFiles(buildLoader(t, qemuTest))}
	if err := image.Disk(disk, contents); err != nil {
		t.Fatal(err)
	}
	return disk
}

func TestTheTestImagePassesOnRealFirmware(t *testing.T) {
	r := newRig(t)
	res, err := Run(context.Background(), r.machine(diskWithLoader(t, true)), Options{Timeout: 2 * time.Minute, AnswerPress: true})
	if err != nil {
		t.Fatalf("run: %v\n%s", err, res.Serial)
	}
	report := ParseReport(res.Serial)
	if err := report.Verdict(res.ExitCode, res.TimedOut, LastLine(res.Serial)); err != nil {
		t.Fatalf("%v\n%s", err, res.Serial)
	}
	for _, want := range []string{"uefi.vendor", "uefi.keyboard.enter", "uefi.keyboard.arrow", "uefi.console.write", "boot.uefi_version"} {
		found := false
		for _, p := range report.Passed {
			found = found || p == want
		}
		if !found {
			t.Errorf("check %s did not run; passed: %v", want, report.Passed)
		}
	}
	if !strings.Contains(res.Serial, consoleProbe) {
		t.Errorf("the console probe did not reach the screen whole:\n%s", res.Serial)
	}
	if !strings.Contains(res.Serial, "Meridian boot ") {
		t.Errorf("no banner in:\n%s", res.Serial)
	}
}

func TestAScriptPressesKeysOnTheMachine(t *testing.T) {
	r := newRig(t)
	m := r.machine(diskWithLoader(t, false))
	m.DebugExit = false
	res, err := Run(context.Background(), m, Options{
		Timeout: 2 * time.Minute,
		Script: []Step{
			{Wait: "Press any key to return to the firmware.", Keys: []string{"ret"}},
			{Wait: "Returning to the firmware."},
		},
		StopAfterScript: true,
	})
	if err != nil {
		t.Fatalf("run: %v\n%s", err, res.Serial)
	}
	if res.StepsDone != 2 || !res.Stopped || res.TimedOut {
		t.Fatalf("steps %d, stopped %v, timed out %v\n%s", res.StepsDone, res.Stopped, res.TimedOut, res.Serial)
	}
}

func TestADiskWithoutLoaderTimesOut(t *testing.T) {
	r := newRig(t)
	disk := filepath.Join(t.TempDir(), "empty.img")
	if err := image.Disk(disk, image.Contents{Seed: "empty", Time: time.Unix(0, 0), Files: map[string][]byte{"README.TXT": []byte("nothing to boot\n")}}); err != nil {
		t.Fatal(err)
	}
	var live strings.Builder
	res, err := Run(context.Background(), r.machine(disk), Options{Timeout: 20 * time.Second, Live: &live})
	if err != nil {
		t.Fatalf("run: %v", err)
	}
	if !res.TimedOut || res.ExitCode != 0 {
		t.Fatalf("timed out %v, exit %d", res.TimedOut, res.ExitCode)
	}
	if live.Len() == 0 || Clean(live.String()) != res.Serial {
		t.Fatalf("the live copy must match what was read: %d bytes live, %d cleaned", live.Len(), len(res.Serial))
	}
	verdict := ParseReport(res.Serial).Verdict(res.ExitCode, res.TimedOut, LastLine(res.Serial))
	if verdict == nil || !strings.Contains(verdict.Error(), "never reported a test") {
		t.Fatalf("verdict: %v", verdict)
	}
}

func TestQEMURefusingItsArgumentsIsReported(t *testing.T) {
	r := newRig(t)
	disk := filepath.Join(t.TempDir(), "x.img")
	if err := os.WriteFile(disk, make([]byte, 1<<20), 0o644); err != nil {
		t.Fatal(err)
	}
	m := r.machine(disk)
	m.Accels = []string{"no-such-accelerator"}
	_, err := Run(context.Background(), m, Options{Timeout: time.Minute})
	if err == nil || !strings.Contains(err.Error(), "QEMU failed with exit code 1") || !strings.Contains(err.Error(), "no-such-accelerator") {
		t.Fatalf("got %v", err)
	}
}

func TestRunChecksTheMachineFirst(t *testing.T) {
	_, err := Run(context.Background(), Machine{}, Options{Timeout: time.Second})
	if err == nil || !strings.Contains(err.Error(), "no QEMU binary given") {
		t.Fatalf("got %v", err)
	}
}

func TestAMissingBinaryIsReported(t *testing.T) {
	dir := t.TempDir()
	vars := filepath.Join(dir, "vars.fd")
	if err := os.WriteFile(vars, []byte{0}, 0o644); err != nil {
		t.Fatal(err)
	}
	m := Machine{Binary: filepath.Join(dir, "no-qemu"), Firmware: Firmware{Code: vars, Vars: vars}, Memory: 64 << 20, CPUs: 1, Accels: []string{"tcg"}, Disk: vars}
	_, err := Run(context.Background(), m, Options{Timeout: time.Second})
	if err == nil || !strings.HasPrefix(err.Error(), "start ") {
		t.Fatalf("got %v", err)
	}
}

func TestAMissingVariableStoreIsReported(t *testing.T) {
	m := Machine{Binary: "qemu", Firmware: Firmware{Code: "c", Vars: filepath.Join(t.TempDir(), "gone.fd")}, Memory: 64 << 20, CPUs: 1, Accels: []string{"tcg"}, Disk: "d"}
	_, err := Run(context.Background(), m, Options{Timeout: time.Second})
	if err == nil || !strings.HasPrefix(err.Error(), "copy the firmware variable store: ") {
		t.Fatalf("got %v", err)
	}
}

func TestFreeLocalAddress(t *testing.T) {
	addr, err := freeLocalAddress()
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(addr, "127.0.0.1:") || addr == "127.0.0.1:0" {
		t.Fatalf("got %q", addr)
	}
}

// An ESP larger than 32MiB does not fit El Torito's 16-bit sector count,
// so the boot entry says 0 and the firmware has to read the image to its
// end. This boots it to prove OVMF does.
func TestTheISOBootsFromCD(t *testing.T) {
	r := newRig(t)
	iso := filepath.Join(t.TempDir(), "meridian.iso")
	contents := image.Contents{Seed: t.Name(), Time: time.Unix(1_760_000_000, 0), Files: image.LoaderFiles(buildLoader(t, true))}
	if err := image.ISO(iso, contents); err != nil {
		t.Fatal(err)
	}
	m := r.machine("")
	m.Disk, m.CDROM = "", iso
	res, err := Run(context.Background(), m, Options{Timeout: 2 * time.Minute, AnswerPress: true})
	if err != nil {
		t.Fatalf("run: %v\n%s", err, res.Serial)
	}
	if err := ParseReport(res.Serial).Verdict(res.ExitCode, res.TimedOut, LastLine(res.Serial)); err != nil {
		t.Fatalf("%v\n%s", err, res.Serial)
	}
}

// A test image waits for its keys. Without answers it hangs until the
// timeout, which shows the keyboard checks really depend on the runner.
func TestUnansweredKeyPressesHang(t *testing.T) {
	r := newRig(t)
	res, err := Run(context.Background(), r.machine(diskWithLoader(t, true)), Options{Timeout: 15 * time.Second})
	if err != nil {
		t.Fatalf("run: %v", err)
	}
	if !res.TimedOut || !strings.Contains(res.Serial, "PRESS ret") || strings.Contains(res.Serial, "uefi.keyboard.enter") {
		t.Fatalf("timed out %v\n%s", res.TimedOut, res.Serial)
	}
}

// Each step looks after the text the previous step waited for, so waiting
// twice for text that appears once must not finish the script.
func TestAScriptNeedsTextToAppearAgain(t *testing.T) {
	r := newRig(t)
	res, err := Run(context.Background(), r.machine(diskWithLoader(t, true)), Options{
		Timeout:     2 * time.Minute,
		AnswerPress: true,
		Script:      []Step{{Wait: "TESTS DONE"}, {Wait: "TESTS DONE"}},
	})
	if err != nil {
		t.Fatalf("run: %v", err)
	}
	if res.StepsDone != 1 || res.ExitCode != ExitPass {
		t.Fatalf("steps %d, exit %d", res.StepsDone, res.ExitCode)
	}
}

func TestStopAfterAnEmptyScriptDoesNotStop(t *testing.T) {
	r := newRig(t)
	res, err := Run(context.Background(), r.machine(diskWithLoader(t, true)), Options{Timeout: 2 * time.Minute, AnswerPress: true, StopAfterScript: true})
	if err != nil {
		t.Fatalf("run: %v", err)
	}
	if res.Stopped || res.ExitCode != ExitPass {
		t.Fatalf("stopped %v, exit %d", res.Stopped, res.ExitCode)
	}
}

func openFiles(t *testing.T) int {
	t.Helper()
	entries, err := os.ReadDir("/proc/self/fd")
	if err != nil {
		t.Fatal(err)
	}
	return len(entries)
}

// sextant test boots many images in one process, so every run must give
// back its pipes, temporary files and monitor connection.
func TestRunsLeaveNoFilesOpen(t *testing.T) {
	if runtime.GOOS != "linux" {
		t.Skip("counts /proc/self/fd")
	}
	r := newRig(t)
	disk := diskWithLoader(t, true)
	if _, err := freeLocalAddress(); err != nil {
		t.Fatal(err)
	}
	before := openFiles(t)
	for i := 0; i < 2; i++ {
		if _, err := Run(context.Background(), r.machine(disk), Options{Timeout: 2 * time.Minute, AnswerPress: true}); err != nil {
			t.Fatal(err)
		}
	}
	if after := openFiles(t); after != before {
		t.Fatalf("%d files open before two runs, %d after", before, after)
	}
}
