package qemu

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func existing(paths ...string) func(string) bool {
	set := map[string]bool{}
	for _, p := range paths {
		set[p] = true
	}
	return func(p string) bool { return set[p] }
}

func TestEveryKnownLocation(t *testing.T) {
	for goos, list := range knownFirmware {
		for _, want := range list {
			got, err := FindFirmware(goos, "", Firmware{}, existing(want.Code, want.Vars))
			if err != nil || got != want {
				t.Errorf("%s: %+v not found: %v", goos, want, err)
			}
		}
	}
}

func TestArchLinuxComesFirst(t *testing.T) {
	arch := knownFirmware["linux"][0]
	debian := knownFirmware["linux"][1]
	got, err := FindFirmware("linux", "", Firmware{}, existing(arch.Code, arch.Vars, debian.Code, debian.Vars))
	if err != nil || got != arch {
		t.Fatalf("got %+v, %v", got, err)
	}
}

func TestBothFilesMustExist(t *testing.T) {
	arch := knownFirmware["linux"][0]
	debian := knownFirmware["linux"][1]
	got, err := FindFirmware("linux", "", Firmware{}, existing(arch.Code, debian.Code, debian.Vars))
	if err != nil || got != debian {
		t.Fatalf("a code file without its variable store must be skipped; got %+v, %v", got, err)
	}
}

func TestFirmwareNextToTheQEMUBinary(t *testing.T) {
	bin := filepath.Join("opt", "qemu", "bin", "qemu-system-x86_64")
	share := filepath.Join("opt", "qemu", "bin", "..", "share", "qemu")
	want := Firmware{Code: filepath.Join(share, "edk2-x86_64-code.fd"), Vars: filepath.Join(share, "edk2-i386-vars.fd")}
	got, err := FindFirmware("plan9", bin, Firmware{}, existing(want.Code, want.Vars))
	if err != nil || got != want {
		t.Fatalf("got %+v, %v", got, err)
	}
	winBin := filepath.Join("qemu", "qemu-system-x86_64.exe")
	winShare := filepath.Join("qemu", "share")
	want = Firmware{Code: filepath.Join(winShare, "edk2-x86_64-code.fd"), Vars: filepath.Join(winShare, "edk2-i386-vars.fd")}
	got, err = FindFirmware("plan9", winBin, Firmware{}, existing(want.Code, want.Vars))
	if err != nil || got != want {
		t.Fatalf("windows layout: got %+v, %v", got, err)
	}
	if besideBinary("") != nil {
		t.Fatal("no binary, no guesses")
	}
}

func TestConfiguredFirmwareWins(t *testing.T) {
	arch := knownFirmware["linux"][0]
	own := Firmware{Code: "/mine/code.fd", Vars: "/mine/vars.fd"}
	got, err := FindFirmware("linux", "", own, existing(arch.Code, arch.Vars, own.Code, own.Vars))
	if err != nil || got != own {
		t.Fatalf("got %+v, %v", got, err)
	}
}

func TestMissingConfiguredFirmwareIsNamed(t *testing.T) {
	own := Firmware{Code: "/mine/code.fd", Vars: "/mine/vars.fd"}
	_, err := FindFirmware("linux", "", own, existing(own.Code))
	if err == nil || err.Error() != "the firmware file /mine/vars.fd set in qemu.firmware or qemu.firmware-vars does not exist" {
		t.Fatalf("got %v", err)
	}
	_, err = FindFirmware("linux", "", Firmware{Code: "/only/code.fd"}, existing("/only/code.fd"))
	if err == nil || !strings.Contains(err.Error(), "the firmware file  set in") {
		t.Fatalf("a missing second file is reported too, got %v", err)
	}
}

func TestNothingFoundSaysWhereItLookedAndWhatToDo(t *testing.T) {
	_, err := FindFirmware("linux", "/usr/bin/qemu-system-x86_64", Firmware{}, existing())
	if err == nil {
		t.Fatal("expected an error")
	}
	msg := err.Error()
	for _, want := range []string{
		"no OVMF firmware found",
		"/usr/share/edk2/x64/OVMF_CODE.4m.fd",
		"/usr/share/edk2/ovmf/OVMF_CODE.fd",
		filepath.Join("/usr/bin", "..", "share", "qemu", "edk2-x86_64-code.fd"),
		"sextant doctor",
		"sextant.local.meri",
	} {
		if !strings.Contains(msg, want) {
			t.Errorf("message misses %q:\n%s", want, msg)
		}
	}
	if _, err := FindFirmware("darwin", "", Firmware{}, existing()); err == nil || !strings.Contains(err.Error(), "/opt/homebrew/share/qemu/edk2-x86_64-code.fd") {
		t.Errorf("macOS locations missing: %v", err)
	}
}

func TestFileExists(t *testing.T) {
	dir := t.TempDir()
	file := filepath.Join(dir, "code.fd")
	if err := os.WriteFile(file, []byte{0}, 0o644); err != nil {
		t.Fatal(err)
	}
	if !FileExists(file) {
		t.Error("an existing file exists")
	}
	if FileExists(dir) {
		t.Error("a directory is not a firmware file")
	}
	if FileExists(filepath.Join(dir, "missing.fd")) {
		t.Error("a missing file does not exist")
	}
}
