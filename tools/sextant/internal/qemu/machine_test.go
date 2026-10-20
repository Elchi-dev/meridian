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
		"-device pcie-root-port,id=meridian.port,chassis=1,slot=1 -device virtio-rng-pci,bus=meridian.port " +
		"-drive if=none,id=meridian.nvme,format=raw,readonly=on,file.locking=off,file=/out/meridian.img -device nvme,serial=meridian,drive=meridian.nvme " +
		"-drive if=none,id=meridian.virtio,format=raw,readonly=on,file.locking=off,file=/out/meridian.img -device virtio-blk-pci,drive=meridian.virtio " +
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

func TestATestMachineWithoutADiskHasNoNVMe(t *testing.T) {
	m := fullMachine()
	m.Disk, m.CDROM = "", "/out/meridian.iso"
	got := strings.Join(m.Args(), " ")
	if strings.Contains(got, "nvme") || strings.Contains(got, "virtio-blk") || !strings.Contains(got, "virtio-rng-pci") {
		t.Fatalf("got %s", got)
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

func TestSecureBootFirmwareGetsSMMAndAProtectedFlash(t *testing.T) {
	m := fullMachine()
	m.SecureBoot = true
	got := strings.Join(m.Args(), " ")
	want := "-machine q35,smm=on -cpu max"
	if !strings.HasPrefix(got, want) {
		t.Fatalf("got %s", got)
	}
	secure := "-global driver=cfi.pflash01,property=secure,value=on -drive if=pflash,format=raw,readonly=on"
	if !strings.Contains(got, secure) {
		t.Fatalf("the flash should be protected before the firmware is added: %s", got)
	}
	plain := strings.Join(fullMachine().Args(), " ")
	if strings.Contains(plain, "smm") || strings.Contains(plain, "secure") {
		t.Fatalf("an ordinary machine needs neither: %s", plain)
	}
}

func TestAMachineCanLackTheX2APIC(t *testing.T) {
	m := fullMachine()
	m.WithoutX2APIC = true
	if got := strings.Join(m.Args(), " "); !strings.HasPrefix(got, "-machine q35 -cpu max,-x2apic -m ") {
		t.Fatalf("got %s", got)
	}
	if got := strings.Join(fullMachine().Args(), " "); !strings.HasPrefix(got, "-machine q35 -cpu max -m ") {
		t.Fatalf("an ordinary machine keeps its x2APIC: %s", got)
	}
}

func TestAMachineCanLackXSAVE(t *testing.T) {
	m := fullMachine()
	m.WithoutXSAVE = true
	if got := strings.Join(m.Args(), " "); !strings.HasPrefix(got, "-machine q35 -cpu max,-xsave -m ") {
		t.Fatalf("got %s", got)
	}
	m.WithoutX2APIC = true
	if got := strings.Join(m.Args(), " "); !strings.HasPrefix(got, "-machine q35 -cpu max,-x2apic,-xsave -m ") {
		t.Fatalf("without either: %s", got)
	}
	if got := strings.Join(fullMachine().Args(), " "); strings.Contains(got, "xsave") {
		t.Fatalf("an ordinary machine keeps XSAVE: %s", got)
	}
}

func TestAMachineCanLackPS2(t *testing.T) {
	m := fullMachine()
	m.WithoutPS2 = true
	got := strings.Join(m.Args(), " ")
	if !strings.HasPrefix(got, "-machine q35,i8042=off -cpu max -m ") {
		t.Fatalf("got %s", got)
	}
	if !strings.Contains(got, "-serial stdio -device qemu-xhci -device usb-kbd -drive if=pflash") {
		t.Fatalf("the keyboard should be a USB one: %s", got)
	}
	m.SecureBoot = true
	if got := strings.Join(m.Args(), " "); !strings.HasPrefix(got, "-machine q35,smm=on,i8042=off ") {
		t.Fatalf("with Secure Boot: %s", got)
	}
	plain := strings.Join(fullMachine().Args(), " ")
	if strings.Contains(plain, "i8042") || strings.Contains(plain, "usb") {
		t.Fatalf("an ordinary machine keeps its PS/2 keyboard: %s", plain)
	}
}

func TestASecondDiskComesAfterTheFirst(t *testing.T) {
	m := fullMachine()
	m.SecondDisk = "/out/other,disk.img"
	got := strings.Join(m.Args(), " ")
	want := "-drive format=raw,file=/out/meridian.img -drive format=raw,file=/out/other,,disk.img -display none"
	if !strings.Contains(got, want) {
		t.Fatalf("got %s", got)
	}
	if err := m.Validate(); err != nil {
		t.Fatalf("two disks are a valid machine: %v", err)
	}
	m.Disk, m.CDROM = "", "x.iso"
	if err := m.Validate(); err == nil || err.Error() != "a second disk needs a first one" {
		t.Fatalf("a second disk beside a CD: %v", err)
	}
	if strings.Count(strings.Join(fullMachine().Args(), " "), "format=raw,file=") != 2 {
		t.Fatal("one disk and the variable store, no empty second drive")
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
