// Package qemu starts Meridian images in QEMU, reads what they report over
// the serial port and types on their keyboard through the QEMU monitor.
package qemu

import (
	"errors"
	"fmt"
	"strconv"
	"strings"
)

// Firmware is an OVMF build: the code, which is read-only, and the variable
// store, of which every run gets its own writable copy.
type Firmware struct {
	Code string
	Vars string
}

// Machine describes one QEMU x86_64 machine: a q35 board with UEFI firmware.
type Machine struct {
	Binary   string
	Firmware Firmware
	Memory   int64
	CPUs     int
	Accels   []string
	// Disk is a raw disk image, CDROM an ISO image. One of them is set.
	Disk  string
	CDROM string
	// SecondDisk is another raw disk image after Disk, for systems on
	// another drive.
	SecondDisk string
	// Display opens a window. Without it the machine still has a VGA card,
	// so the firmware offers a framebuffer, but nothing is shown.
	Display bool
	// DebugExit makes it a machine for test images: it adds isa-debug-exit
	// on port 0xf4, which they end QEMU with to give their verdict, a PCIe
	// root port with a random number generator behind it, for their checks
	// of devices on a bus of their own, and with a disk the same disk once
	// more as NVMe, read only, for their checks of NVMe. OVMF finds that
	// one first and boots from it.
	DebugExit bool
	// Monitor is a QEMU character device for the human monitor, such as
	// "tcp:127.0.0.1:4444,server=on,wait=off". Empty means no monitor.
	Monitor string
	// GDB makes the machine wait for a debugger on TCP port 1234.
	GDB bool
	// Reboot lets a reset start the machine again. Without it QEMU ends
	// when the machine resets, so a crash that resets cannot loop.
	Reboot bool
	// SecureBoot runs the firmware the way OVMF's Secure Boot builds need:
	// with system management mode, and with the flash holding the
	// variables writable only from it, so nothing outside the firmware can
	// change the keys.
	SecureBoot bool
	// WithoutX2APIC hides the x2APIC from the processor, so it has only
	// the older xAPIC, as processors from before about 2013 do.
	WithoutX2APIC bool
	// WithoutXSAVE hides XSAVE from the processor, so it keeps its
	// registers with FXSAVE only, as processors from before about 2008 do.
	WithoutXSAVE bool
	// WithoutPS2 leaves the 8042 out, so the keyboard is a USB one behind
	// an xHCI controller, as on PCs without PS/2 ports.
	WithoutPS2 bool
	// WritingCopies lets the test machine's NVMe and virtio disks write
	// too: into copies of the disk QEMU keeps until it ends, so what the
	// AHCI disk wrote stays as it was. Without it they only read.
	WritingCopies bool
}

// Exit codes of isa-debug-exit: QEMU exits with (value << 1) | 1, and test
// images write 0x10 for a pass and 0x11 for a failure.
const (
	ExitPass = 0x10<<1 | 1
	ExitFail = 0x11<<1 | 1
)

// Validate refuses machines QEMU would refuse later with a less helpful
// message.
func (m Machine) Validate() error {
	var errs []error
	if m.Binary == "" {
		errs = append(errs, errors.New("no QEMU binary given"))
	}
	if m.Firmware.Code == "" || m.Firmware.Vars == "" {
		errs = append(errs, errors.New("both firmware files are needed: code and variable store"))
	}
	if (m.Disk == "") == (m.CDROM == "") {
		errs = append(errs, errors.New("give exactly one of a disk image and an ISO image"))
	}
	if m.SecondDisk != "" && m.Disk == "" {
		errs = append(errs, errors.New("a second disk needs a first one"))
	}
	if m.Memory < 1<<20 {
		errs = append(errs, fmt.Errorf("memory of %d bytes is less than 1MiB", m.Memory))
	}
	if m.CPUs < 1 {
		errs = append(errs, fmt.Errorf("a machine needs at least one CPU, got %d", m.CPUs))
	}
	if len(m.Accels) == 0 {
		errs = append(errs, errors.New("no accelerator given; use tcg to emulate"))
	}
	return errors.Join(errs...)
}

// Args is the QEMU command line, without the binary.
func (m Machine) Args() []string {
	board := "q35"
	if m.SecureBoot {
		board = "q35,smm=on"
	}
	if m.WithoutPS2 {
		board += ",i8042=off"
	}
	cpu := "max"
	if m.WithoutX2APIC {
		cpu += ",-x2apic"
	}
	if m.WithoutXSAVE {
		cpu += ",-xsave"
	}
	args := []string{
		"-machine", board,
		"-cpu", cpu,
		"-m", strconv.FormatInt(m.Memory>>20, 10) + "M",
		"-smp", strconv.Itoa(m.CPUs),
	}
	for _, accel := range m.Accels {
		args = append(args, "-accel", accel)
	}
	args = append(args, "-nodefaults")
	if !m.Reboot {
		args = append(args, "-no-reboot")
	}
	args = append(args,
		"-vga", "std",
		"-serial", "stdio",
	)
	if m.WithoutPS2 {
		args = append(args, "-device", "qemu-xhci", "-device", "usb-kbd")
	}
	if m.SecureBoot {
		args = append(args, "-global", "driver=cfi.pflash01,property=secure,value=on")
	}
	args = append(args,
		"-drive", "if=pflash,format=raw,readonly=on,file="+escape(m.Firmware.Code),
		"-drive", "if=pflash,format=raw,file="+escape(m.Firmware.Vars),
	)
	for _, disk := range []string{m.Disk, m.SecondDisk} {
		if disk != "" {
			args = append(args, "-drive", "format=raw,file="+escape(disk))
		}
	}
	if m.CDROM != "" {
		args = append(args, "-drive", "media=cdrom,format=raw,readonly=on,file="+escape(m.CDROM))
	}
	if !m.Display {
		args = append(args, "-display", "none")
	}
	if m.DebugExit {
		args = append(args,
			"-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
			"-device", "pcie-root-port,id=meridian.port,chassis=1,slot=1",
			"-device", "virtio-rng-pci,bus=meridian.port",
		)
		if m.Disk != "" {
			// The disk is open for writing already; these looks at it
			// write only to copies if at all, so they do not ask for the
			// lock.
			mode := "readonly=on"
			if m.WritingCopies {
				mode = "snapshot=on"
			}
			args = append(args,
				"-drive", "if=none,id=meridian.nvme,format=raw,"+mode+",file.locking=off,file="+escape(m.Disk),
				"-device", "nvme,serial=meridian,drive=meridian.nvme",
				"-drive", "if=none,id=meridian.virtio,format=raw,"+mode+",file.locking=off,file="+escape(m.Disk),
				"-device", "virtio-blk-pci,drive=meridian.virtio",
			)
		}
	}
	if m.Monitor != "" {
		args = append(args, "-monitor", m.Monitor)
	}
	if m.GDB {
		args = append(args, "-s", "-S")
	}
	return args
}

// escape doubles commas, which QEMU otherwise reads as the end of a file
// name inside -drive options.
func escape(path string) string {
	return strings.ReplaceAll(path, ",", ",,")
}

// Accelerators picks what to ask QEMU for. QEMU tries them in order and
// falls back to the next when one is not available, so "auto" always ends
// with TCG, which emulates every instruction and works everywhere.
func Accelerators(configured, goos, goarch string) []string {
	if configured != "auto" {
		return []string{configured}
	}
	switch {
	case goos == "linux":
		return []string{"kvm", "tcg"}
	case goos == "darwin" && goarch == "amd64":
		return []string{"hvf", "tcg"}
	case goos == "windows":
		return []string{"whpx", "tcg"}
	}
	return []string{"tcg"}
}
