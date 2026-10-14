package qemu

import (
	"strings"
	"testing"
)

func fullMachine() Machine {
	return Machine{
		Binary:    "qemu-system-x86_64",
		Firmware:  Firmware{Code: "/fw/code.fd", Vars: "/run/vars.fd"},
		Memory:    512 << 20,
		CPUs:      4,
		Accels:    []string{"kvm", "tcg"},
		Disk:      "/out/meridian.img",
		DebugExit: true,
		Monitor:   "tcp:127.0.0.1:4444,server=on,wait=off",
		GDB:       true,
	}
}

func TestArgsForAFullMachine(t *testing.T) {
	got := strings.Join(fullMachine().Args(), " ")
	want := "-machine q35 -cpu max -m 512M -smp 4 -accel kvm -accel tcg -nodefaults -no-reboot -vga std -serial stdio " +
		"-drive if=pflash,format=raw,readonly=on,file=/fw/code.fd -drive if=pflash,format=raw,file=/run/vars.fd " +
		"-drive format=raw,file=/out/meridian.img -display none -device isa-debug-exit,iobase=0xf4,iosize=0x04 " +
		"-monitor tcp:127.0.0.1:4444,server=on,wait=off -s -S"
	if got != want {
		t.Fatalf("got\n%s\nwant\n%s", got, want)
	}
}

func TestArgsForAMinimalMachineWithAWindow(t *testing.T) {
	m := Machine{
		Binary:   "qemu",
		Firmware: Firmware{Code: "c", Vars: "v"},
		Memory:   2 << 30,
		CPUs:     1,
		Accels:   []string{"tcg"},
		CDROM:    "/out/meridian.iso",
		Display:  true,
	}
	got := strings.Join(m.Args(), " ")
	want := "-machine q35 -cpu max -m 2048M -smp 1 -accel tcg -nodefaults -no-reboot -vga std -serial stdio " +
		"-drive if=pflash,format=raw,readonly=on,file=c -drive if=pflash,format=raw,file=v " +
		"-drive media=cdrom,format=raw,readonly=on,file=/out/meridian.iso"
	if got != want {
		t.Fatalf("got\n%s\nwant\n%s", got, want)
	}
}

func TestAMachineThatRebootsKeepsRunningAfterAReset(t *testing.T) {
	m := fullMachine()
	m.Reboot = true
	got := strings.Join(m.Args(), " ")
	if strings.Contains(got, "-no-reboot") || !strings.Contains(got, "-nodefaults -vga std") {
		t.Fatalf("got %s", got)
	}
}

func TestCommasInPathsAreDoubled(t *testing.T) {
	m := fullMachine()
	m.Disk = "/tmp/a,b/disk.img"
	m.Firmware.Code = "C:\\fw,1\\code.fd"
	args := strings.Join(m.Args(), " ")
	for _, want := range []string{"file=/tmp/a,,b/disk.img", "file=C:\\fw,,1\\code.fd"} {
		if !strings.Contains(args, want) {
			t.Errorf("%q missing from %s", want, args)
		}
	}
	if escape("no commas") != "no commas" || escape(",,") != ",,,," {
		t.Error("escape changes only commas")
	}
}

func TestValidate(t *testing.T) {
	if err := fullMachine().Validate(); err != nil {
		t.Fatalf("a full machine is valid: %v", err)
	}
	tests := []struct {
		name   string
		change func(*Machine)
		want   string
	}{
		{"binary", func(m *Machine) { m.Binary = "" }, "no QEMU binary given"},
		{"code", func(m *Machine) { m.Firmware.Code = "" }, "both firmware files are needed"},
		{"vars", func(m *Machine) { m.Firmware.Vars = "" }, "both firmware files are needed"},
		{"no media", func(m *Machine) { m.Disk = "" }, "give exactly one of a disk image and an ISO image"},
		{"two media", func(m *Machine) { m.CDROM = "x.iso" }, "give exactly one of a disk image and an ISO image"},
		{"memory", func(m *Machine) { m.Memory = 1<<20 - 1 }, "memory of 1048575 bytes is less than 1MiB"},
		{"cpus", func(m *Machine) { m.CPUs = 0 }, "a machine needs at least one CPU, got 0"},
		{"accels", func(m *Machine) { m.Accels = nil }, "no accelerator given; use tcg to emulate"},
	}
	for _, tt := range tests {
		m := fullMachine()
		tt.change(&m)
		err := m.Validate()
		if err == nil || !strings.Contains(err.Error(), tt.want) {
			t.Errorf("%s: got %v, want %q", tt.name, err, tt.want)
		}
	}
	m := fullMachine()
	m.Memory = 1 << 20
	m.CPUs = 1
	if err := m.Validate(); err != nil {
		t.Errorf("1MiB and one CPU are the smallest valid machine: %v", err)
	}
}

func TestValidateReportsEverything(t *testing.T) {
	err := Machine{}.Validate()
	if err == nil || strings.Count(err.Error(), "\n") != 5 {
		t.Fatalf("an empty machine has six problems, got %v", err)
	}
}

func TestAccelerators(t *testing.T) {
	tests := []struct {
		configured, goos, goarch string
		want                     string
	}{
		{"auto", "linux", "amd64", "kvm tcg"},
		{"auto", "linux", "arm64", "kvm tcg"},
		{"auto", "darwin", "amd64", "hvf tcg"},
		{"auto", "darwin", "arm64", "tcg"},
		{"auto", "windows", "amd64", "whpx tcg"},
		{"auto", "freebsd", "amd64", "tcg"},
		{"tcg", "linux", "amd64", "tcg"},
		{"kvm", "windows", "amd64", "kvm"},
	}
	for _, tt := range tests {
		if got := strings.Join(Accelerators(tt.configured, tt.goos, tt.goarch), " "); got != tt.want {
			t.Errorf("%s on %s/%s: got %q, want %q", tt.configured, tt.goos, tt.goarch, got, tt.want)
		}
	}
}

func TestExitCodes(t *testing.T) {
	if ExitPass != 33 || ExitFail != 35 {
		t.Fatalf("the test images write 0x10 and 0x11; QEMU turns them into 33 and 35, got %d and %d", ExitPass, ExitFail)
	}
}
