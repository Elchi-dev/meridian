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
	// Display opens a window. Without it the machine still has a VGA card,
	// so the firmware offers a framebuffer, but nothing is shown.
	Display bool
	// DebugExit adds isa-debug-exit on port 0xf4, which test images use to
	// end QEMU with their verdict.
	DebugExit bool
	// Monitor is a QEMU character device for the human monitor, such as
	// "tcp:127.0.0.1:4444,server=on,wait=off". Empty means no monitor.
	Monitor string
	// GDB makes the machine wait for a debugger on TCP port 1234.
	GDB bool
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
	args := []string{
		"-machine", "q35",
		"-cpu", "max",
		"-m", strconv.FormatInt(m.Memory>>20, 10) + "M",
		"-smp", strconv.Itoa(m.CPUs),
	}
	for _, accel := range m.Accels {
		args = append(args, "-accel", accel)
	}
	args = append(args,
		"-nodefaults",
		"-no-reboot",
		"-vga", "std",
		"-serial", "stdio",
		"-drive", "if=pflash,format=raw,readonly=on,file="+escape(m.Firmware.Code),
		"-drive", "if=pflash,format=raw,file="+escape(m.Firmware.Vars),
	)
	if m.Disk != "" {
		args = append(args, "-drive", "format=raw,file="+escape(m.Disk))
	}
	if m.CDROM != "" {
		args = append(args, "-drive", "media=cdrom,format=raw,readonly=on,file="+escape(m.CDROM))
	}
	if !m.Display {
		args = append(args, "-display", "none")
	}
	if m.DebugExit {
		args = append(args, "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04")
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
