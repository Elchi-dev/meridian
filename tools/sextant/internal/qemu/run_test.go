package qemu

import (
	"context"
	"image/png"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
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

// reliable adds the restart for firmware that stalls before printing
// anything, which OVMF under KVM does now and then on some hosts.
func reliable(o Options) Options {
	o.FirmwareStall = 20 * time.Second
	o.Attempts = 3
	return o
}

// bootFiles builds the real loader and kernel with Cargo and returns the
// files of the EFI system partition.
func bootFiles(t *testing.T, qemuTest bool) map[string][]byte {
	t.Helper()
	read := func(part cargo.Part) []byte {
		b := cargo.Build{Part: part, Root: repoRoot(t), BuildDir: filepath.Join(os.TempDir(), "sextant-test-build"), QEMUTest: qemuTest}
		path, err := b.Run(context.Background(), io.Discard, io.Discard)
		if err != nil {
			t.Fatalf("build %s: %v", part.Package, err)
		}
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		return data
	}
	return image.BootFiles(read(cargo.Loader), read(cargo.Kernel))
}

// diskWithLoader writes the real loader onto a disk image.
func diskWithLoader(t *testing.T, qemuTest bool) string {
	t.Helper()
	disk := filepath.Join(t.TempDir(), "meridian.img")
	contents := image.Contents{Seed: t.Name(), Time: time.Unix(1_760_000_000, 0), Files: bootFiles(t, qemuTest)}
	if err := image.Disk(disk, contents); err != nil {
		t.Fatal(err)
	}
	return disk
}

func TestTheTestImagePassesOnRealFirmware(t *testing.T) {
	r := newRig(t)
	screens := t.TempDir()
	res, err := Run(context.Background(), r.machine(diskWithLoader(t, true)), reliable(Options{Timeout: 2 * time.Minute, AnswerPress: true, Screens: screens}))
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
	checkStatusScreen(t, res.Screens, filepath.Join(screens, "status.png"))
	if !strings.Contains(res.Serial, consoleProbe) {
		t.Errorf("the console probe did not reach the screen whole:\n%s", res.Serial)
	}
	if !strings.Contains(res.Serial, "Meridian boot ") {
		t.Errorf("no banner in:\n%s", res.Serial)
	}
}

// The test image asks for keys with PRESS lines; here the script answers
// them instead of AnswerPress, so the keys really come from the script.
func TestAScriptPressesKeysOnTheMachine(t *testing.T) {
	r := newRig(t)
	res, err := Run(context.Background(), r.machine(diskWithLoader(t, true)), reliable(Options{
		Timeout: 2 * time.Minute,
		Script: []Step{
			{Wait: "PRESS ret", Keys: []string{"ret"}},
			{Wait: "PRESS down", Keys: []string{"down"}},
			{Wait: "TEST uefi.keyboard.arrow ok"},
		},
		StopAfterScript: true,
	}))
	if err != nil {
		t.Fatalf("run: %v\n%s", err, res.Serial)
	}
	if res.StepsDone != 3 || !res.Stopped || res.TimedOut {
		t.Fatalf("steps %d, stopped %v, timed out %v\n%s", res.StepsDone, res.Stopped, res.TimedOut, res.Serial)
	}
	if !strings.Contains(res.Serial, "TEST uefi.keyboard.enter ok") {
		t.Fatalf("the script's Enter did not arrive:\n%s", res.Serial)
	}
}

// The image people boot starts the kernel, which says hello and halts.
func TestThePlainImageStartsTheKernel(t *testing.T) {
	r := newRig(t)
	m := r.machine(diskWithLoader(t, false))
	m.DebugExit = false
	res, err := Run(context.Background(), m, reliable(Options{
		Timeout: 2 * time.Minute,
		Script: []Step{
			{Wait: "Meridian boot "},
			{Wait: "Meridian kernel "},
			{Wait: "There is nothing more to do yet. Halting."},
		},
		StopAfterScript: true,
	}))
	if err != nil {
		t.Fatalf("run: %v\n%s", err, res.Serial)
	}
	if res.StepsDone != 3 || !res.Stopped || res.TimedOut {
		t.Fatalf("steps %d, stopped %v, timed out %v\n%s", res.StepsDone, res.Stopped, res.TimedOut, res.Serial)
	}
	if strings.Contains(res.Serial, "TEST ") {
		t.Fatalf("the plain image ran checks:\n%s", res.Serial)
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
	contents := image.Contents{Seed: t.Name(), Time: time.Unix(1_760_000_000, 0), Files: bootFiles(t, true)}
	if err := image.ISO(iso, contents); err != nil {
		t.Fatal(err)
	}
	m := r.machine("")
	m.Disk, m.CDROM = "", iso
	res, err := Run(context.Background(), m, reliable(Options{Timeout: 2 * time.Minute, AnswerPress: true}))
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
	res, err := Run(context.Background(), r.machine(diskWithLoader(t, true)), Options{Timeout: 15 * time.Second, FirmwareStall: 8 * time.Second, Attempts: 3})
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
	res, err := Run(context.Background(), r.machine(diskWithLoader(t, true)), reliable(Options{
		Timeout:     2 * time.Minute,
		AnswerPress: true,
		Script:      []Step{{Wait: "TESTS DONE"}, {Wait: "TESTS DONE"}},
	}))
	if err != nil {
		t.Fatalf("run: %v", err)
	}
	if res.StepsDone != 1 || res.ExitCode != ExitPass {
		t.Fatalf("steps %d, exit %d", res.StepsDone, res.ExitCode)
	}
}

func TestStopAfterAnEmptyScriptDoesNotStop(t *testing.T) {
	r := newRig(t)
	res, err := Run(context.Background(), r.machine(diskWithLoader(t, true)), reliable(Options{Timeout: 2 * time.Minute, AnswerPress: true, StopAfterScript: true}))
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
		if _, err := Run(context.Background(), r.machine(disk), reliable(Options{Timeout: 2 * time.Minute, AnswerPress: true})); err != nil {
			t.Fatal(err)
		}
	}
	if after := openFiles(t); after != before {
		t.Fatalf("%d files open before two runs, %d after", before, after)
	}
}

// A machine whose CPU never starts prints nothing at all, which is exactly
// what a stalled firmware looks like from outside.
func TestAStalledMachineIsStartedAgain(t *testing.T) {
	r := newRig(t)
	m := r.machine(diskWithLoader(t, true))
	m.GDB = true
	var live strings.Builder
	res, err := Run(context.Background(), m, Options{Timeout: time.Minute, Live: &live, FirmwareStall: 2 * time.Second, Attempts: 2})
	if err != nil {
		t.Fatalf("run: %v", err)
	}
	if !res.FirmwareStalled || res.Attempts != 2 || res.TimedOut {
		t.Fatalf("stalled %v, attempts %d, timed out %v", res.FirmwareStalled, res.Attempts, res.TimedOut)
	}
	if !strings.Contains(live.String(), "sextant: the firmware printed nothing for 2s; starting the machine again (2 of 2)") {
		t.Fatalf("the restart must be visible, got %q", live.String())
	}
	once, err := Run(context.Background(), m, Options{Timeout: time.Minute, FirmwareStall: 2 * time.Second})
	if err != nil || !once.FirmwareStalled || once.Attempts != 1 {
		t.Fatalf("without Attempts a stalled machine is started once: %+v, %v", once, err)
	}
}

func TestFirmwareThatPrintsIsNotStalled(t *testing.T) {
	r := newRig(t)
	res, err := Run(context.Background(), r.machine(diskWithLoader(t, true)), Options{Timeout: 2 * time.Minute, AnswerPress: true, FirmwareStall: 20 * time.Second, Attempts: 3})
	if err != nil {
		t.Fatalf("run: %v", err)
	}
	if res.FirmwareStalled || res.ExitCode != ExitPass {
		t.Fatalf("stalled %v, exit %d", res.FirmwareStalled, res.ExitCode)
	}
}

// processWith returns the process whose command line contains needle, or 0.
func processWith(needle string) int {
	entries, _ := os.ReadDir("/proc")
	for _, e := range entries {
		pid, err := strconv.Atoi(e.Name())
		if err != nil {
			continue
		}
		cmdline, err := os.ReadFile(filepath.Join("/proc", e.Name(), "cmdline"))
		if err == nil && strings.Contains(string(cmdline), needle) && strings.HasPrefix(string(cmdline), "/") {
			return pid
		}
	}
	return 0
}

// TestHelperHoldsAMachine only runs as the child process of the test below:
// it starts a machine whose CPU never runs and waits for it.
func TestHelperHoldsAMachine(t *testing.T) {
	disk := os.Getenv("SEXTANT_HELPER_DISK")
	if disk == "" {
		t.Skip("runs only as a helper process")
	}
	m := newRig(t).machine(disk)
	m.GDB = true
	_, _ = Run(context.Background(), m, Options{Timeout: time.Minute})
}

// sextant test boots machines from processes that a timeout may kill. The
// machines must die with them, or they keep running and holding their disk
// images, as eight of them once did on Samuel's machine.
func TestQEMUDiesWithTheProcessThatStartedIt(t *testing.T) {
	if runtime.GOOS != "linux" {
		t.Skip("only Linux can tie a child's life to its parent")
	}
	newRig(t)
	disk := filepath.Join(t.TempDir(), "orphan-check.img")
	if err := os.WriteFile(disk, make([]byte, 1<<20), 0o644); err != nil {
		t.Fatal(err)
	}
	helper := exec.Command(os.Args[0], "-test.run=^TestHelperHoldsAMachine$")
	helper.Env = append(os.Environ(), "SEXTANT_HELPER_DISK="+disk)
	if err := helper.Start(); err != nil {
		t.Fatal(err)
	}
	var machine int
	for i := 0; i < 300 && machine == 0; i++ {
		time.Sleep(100 * time.Millisecond)
		machine = processWith(disk)
	}
	if machine == 0 {
		_ = helper.Process.Kill()
		t.Fatal("the helper never started QEMU")
	}
	if err := helper.Process.Kill(); err != nil {
		t.Fatal(err)
	}
	_ = helper.Wait()
	for i := 0; i < 50; i++ {
		if processWith(disk) == 0 {
			return
		}
		time.Sleep(100 * time.Millisecond)
	}
	if p, err := os.FindProcess(machine); err == nil {
		_ = p.Kill()
	}
	t.Fatalf("QEMU (pid %d) kept running after the process that started it was killed", machine)
}

// checkStatusScreen looks at the picture of the kernel's status screen: the
// dark background in the corners and the orange line under the title.
func checkStatusScreen(t *testing.T, saved []string, want string) {
	t.Helper()
	if len(saved) != 1 || saved[0] != want {
		t.Fatalf("screens saved: %v, want %s", saved, want)
	}
	f, err := os.Open(want)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	img, err := png.Decode(f)
	if err != nil {
		t.Fatal(err)
	}
	b := img.Bounds()
	if b.Dx() < 640 || b.Dy() < 480 {
		t.Fatalf("the screen is %v", b)
	}
	rgb := func(x, y int) [3]uint32 {
		r, g, bl, _ := img.At(x, y).RGBA()
		return [3]uint32{r >> 8, g >> 8, bl >> 8}
	}
	night, tangerine := [3]uint32{0x15, 0x17, 0x1a}, [3]uint32{0xff, 0x96, 0x33}
	for _, at := range [][2]int{{0, 0}, {b.Dx() - 1, b.Dy() - 1}, {b.Dx() - 1, 0}} {
		if got := rgb(at[0], at[1]); got != night {
			t.Errorf("pixel %v is %x, want the dark background", at, got)
		}
	}
	if got := rgb(60, 116); got != tangerine {
		t.Errorf("the line under the title is %x, want tangerine", got)
	}
}
