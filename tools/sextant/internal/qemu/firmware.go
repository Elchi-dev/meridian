package qemu

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// knownFirmware lists where systems install OVMF, for each operating system.
var knownFirmware = map[string][]Firmware{
	"linux": {
		{"/usr/share/edk2/x64/OVMF_CODE.4m.fd", "/usr/share/edk2/x64/OVMF_VARS.4m.fd"},
		{"/usr/share/OVMF/OVMF_CODE_4M.fd", "/usr/share/OVMF/OVMF_VARS_4M.fd"},
		{"/usr/share/OVMF/OVMF_CODE.fd", "/usr/share/OVMF/OVMF_VARS.fd"},
		{"/usr/share/edk2/ovmf/OVMF_CODE.fd", "/usr/share/edk2/ovmf/OVMF_VARS.fd"},
		{"/usr/share/qemu/ovmf-x86_64-code.bin", "/usr/share/qemu/ovmf-x86_64-vars.bin"},
		{"/usr/share/qemu/edk2-x86_64-code.fd", "/usr/share/qemu/edk2-i386-vars.fd"},
	},
	"darwin": {
		{"/opt/homebrew/share/qemu/edk2-x86_64-code.fd", "/opt/homebrew/share/qemu/edk2-i386-vars.fd"},
		{"/usr/local/share/qemu/edk2-x86_64-code.fd", "/usr/local/share/qemu/edk2-i386-vars.fd"},
	},
	"windows": {
		{`C:\Program Files\qemu\share\edk2-x86_64-code.fd`, `C:\Program Files\qemu\share\edk2-i386-vars.fd`},
	},
}

// besideBinary is the copy QEMU ships with itself, found relative to the
// QEMU binary: share/ next to it on Windows, ../share/qemu elsewhere.
func besideBinary(binary string) []Firmware {
	if binary == "" {
		return nil
	}
	dir := filepath.Dir(binary)
	var out []Firmware
	for _, share := range []string{filepath.Join(dir, "share"), filepath.Join(dir, "..", "share", "qemu")} {
		out = append(out, Firmware{
			Code: filepath.Join(share, "edk2-x86_64-code.fd"),
			Vars: filepath.Join(share, "edk2-i386-vars.fd"),
		})
	}
	return out
}

// FindFirmware returns the configured firmware when it exists, or else the
// first known installation where both files exist.
func FindFirmware(goos, binary string, configured Firmware, exists func(string) bool) (Firmware, error) {
	if configured.Code != "" || configured.Vars != "" {
		for _, f := range []string{configured.Code, configured.Vars} {
			if !exists(f) {
				return Firmware{}, fmt.Errorf("the firmware file %s set in qemu.firmware or qemu.firmware-vars does not exist", f)
			}
		}
		return configured, nil
	}
	candidates := append(append([]Firmware(nil), knownFirmware[goos]...), besideBinary(binary)...)
	var looked []string
	for _, c := range candidates {
		if exists(c.Code) && exists(c.Vars) {
			return c, nil
		}
		looked = append(looked, c.Code)
	}
	return Firmware{}, fmt.Errorf("no OVMF firmware found; looked for %s. Run 'sextant doctor' to install it, or set qemu.firmware and qemu.firmware-vars in sextant.local.meri",
		strings.Join(looked, ", "))
}

// FileExists is the exists function for the real file system.
func FileExists(path string) bool {
	info, err := os.Stat(path)
	return err == nil && !info.IsDir()
}
