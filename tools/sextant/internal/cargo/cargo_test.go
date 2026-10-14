package cargo

import (
	"bytes"
	"context"
	"debug/elf"
	"encoding/binary"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestArgsForEveryVariant(t *testing.T) {
	tests := []struct {
		part              Part
		release, qemuTest bool
		want              string
	}{
		{Loader, false, false, "build --locked -p boot --target x86_64-unknown-uefi"},
		{Loader, true, false, "build --locked -p boot --target x86_64-unknown-uefi --release"},
		{Loader, false, true, "build --locked -p boot --target x86_64-unknown-uefi --features qemu-test"},
		{Loader, true, true, "build --locked -p boot --target x86_64-unknown-uefi --release --features qemu-test"},
		{Kernel, false, false, "build --locked -p kernel --target x86_64-unknown-none"},
		{Kernel, true, true, "build --locked -p kernel --target x86_64-unknown-none --release --features qemu-test"},
	}
	for _, tt := range tests {
		b := Build{Part: tt.part, Root: "/repo", BuildDir: "/build", Release: tt.release, QEMUTest: tt.qemuTest}
		if got := strings.Join(b.Args(), " "); got != tt.want {
			t.Errorf("%s, release %v, test %v:\ngot  %s\nwant %s", tt.part.Package, tt.release, tt.qemuTest, got, tt.want)
		}
	}
}

func TestTestImagesGetTheirOwnTargetDirectory(t *testing.T) {
	plain := Build{Part: Kernel, BuildDir: "/build"}
	test := Build{Part: Kernel, BuildDir: "/build", QEMUTest: true}
	if got := plain.Env(); len(got) != 1 || got[0] != "CARGO_TARGET_DIR=/build" {
		t.Errorf("plain env = %v", got)
	}
	if got := test.Env(); len(got) != 1 || got[0] != "CARGO_TARGET_DIR="+filepath.Join("/build", "qemu-test") {
		t.Errorf("test env = %v", got)
	}
}

func TestOutputPaths(t *testing.T) {
	tests := []struct {
		b    Build
		want string
	}{
		{Build{Part: Loader, BuildDir: "/b"}, filepath.Join("/b", "x86_64-unknown-uefi", "debug", "meridian.efi")},
		{Build{Part: Loader, BuildDir: "/b", Release: true}, filepath.Join("/b", "x86_64-unknown-uefi", "release", "meridian.efi")},
		{Build{Part: Loader, BuildDir: "/b", QEMUTest: true}, filepath.Join("/b", "qemu-test", "x86_64-unknown-uefi", "debug", "meridian.efi")},
		{Build{Part: Kernel, BuildDir: "/b"}, filepath.Join("/b", "x86_64-unknown-none", "debug", "kernel")},
		{Build{Part: Kernel, BuildDir: "/b", Release: true, QEMUTest: true}, filepath.Join("/b", "qemu-test", "x86_64-unknown-none", "release", "kernel")},
	}
	for _, tt := range tests {
		if got := tt.b.Output(); got != tt.want {
			t.Errorf("got %s, want %s", got, tt.want)
		}
	}
}

func needCargo(t *testing.T) {
	t.Helper()
	if _, err := exec.LookPath("cargo"); err != nil {
		if os.Getenv("SEXTANT_REQUIRE_TOOLS") == "1" {
			t.Fatal("cargo is required but missing")
		}
		t.Skip("cargo is missing")
	}
}

// isEFIApplication checks the PE headers the firmware checks: the MZ and
// PE signatures, the x86_64 machine type and the EFI application subsystem.
func isEFIApplication(data []byte) string {
	if len(data) < 0x40 || string(data[:2]) != "MZ" {
		return "no MZ header"
	}
	pe := int(binary.LittleEndian.Uint32(data[0x3c:]))
	if pe+24+70 > len(data) || !bytes.Equal(data[pe:pe+4], []byte("PE\x00\x00")) {
		return "no PE signature"
	}
	if machine := binary.LittleEndian.Uint16(data[pe+4:]); machine != 0x8664 {
		return "not built for x86_64"
	}
	optional := pe + 24
	if magic := binary.LittleEndian.Uint16(data[optional:]); magic != 0x20b {
		return "not PE32+"
	}
	if subsystem := binary.LittleEndian.Uint16(data[optional+68:]); subsystem != 10 {
		return "not an EFI application"
	}
	return ""
}

func repoRoot(t *testing.T) string {
	t.Helper()
	root, err := filepath.Abs("../../../..")
	if err != nil {
		t.Fatal(err)
	}
	return root
}

func TestBuildingTheLoader(t *testing.T) {
	needCargo(t)
	b := Build{Part: Loader, Root: repoRoot(t), BuildDir: filepath.Join(os.TempDir(), "sextant-test-build")}
	path, err := b.Run(context.Background(), io.Discard, io.Discard)
	if err != nil {
		t.Fatal(err)
	}
	if path != b.Output() {
		t.Fatalf("built %s, expected %s", path, b.Output())
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if problem := isEFIApplication(data); problem != "" {
		t.Fatalf("%s: %s", path, problem)
	}
}

// The loader reads the kernel as a position independent x86_64 ELF file
// linked into the higher half.
func TestBuildingTheKernel(t *testing.T) {
	needCargo(t)
	b := Build{Part: Kernel, Root: repoRoot(t), BuildDir: filepath.Join(os.TempDir(), "sextant-test-build")}
	path, err := b.Run(context.Background(), io.Discard, io.Discard)
	if err != nil {
		t.Fatal(err)
	}
	f, err := elf.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	if f.Class != elf.ELFCLASS64 || f.Machine != elf.EM_X86_64 || f.Type != elf.ET_DYN {
		t.Fatalf("class %v, machine %v, type %v", f.Class, f.Machine, f.Type)
	}
	if f.Entry < 0xffffffff80000000 {
		t.Fatalf("entry %#x is not in the top 2 GiB", f.Entry)
	}
	var loads int
	for _, p := range f.Progs {
		if p.Type == elf.PT_LOAD {
			loads++
			if p.Vaddr%0x1000 != 0 || p.Flags&elf.PF_W != 0 && p.Flags&elf.PF_X != 0 {
				t.Errorf("segment at %#x: flags %v, not page aligned or writable and executable", p.Vaddr, p.Flags)
			}
		}
	}
	if loads != 3 {
		t.Errorf("%d loadable segments, want code, read-only data and data", loads)
	}
}

func TestCargoFailuresAreReported(t *testing.T) {
	needCargo(t)
	var stderr bytes.Buffer
	b := Build{Part: Loader, Root: t.TempDir(), BuildDir: t.TempDir()}
	_, err := b.Run(context.Background(), io.Discard, &stderr)
	if err == nil || !strings.HasPrefix(err.Error(), "cargo [build --locked -p boot") {
		t.Fatalf("got %v", err)
	}
	if !strings.Contains(stderr.String(), "Cargo.toml") {
		t.Fatalf("cargo's own message should reach stderr, got %q", stderr.String())
	}
}

func TestIsEFIApplicationRejectsOtherFiles(t *testing.T) {
	pe := make([]byte, 0x200)
	copy(pe, "MZ")
	binary.LittleEndian.PutUint32(pe[0x3c:], 0x80)
	copy(pe[0x80:], "PE\x00\x00")
	binary.LittleEndian.PutUint16(pe[0x84:], 0x8664)
	binary.LittleEndian.PutUint16(pe[0x98:], 0x20b)
	binary.LittleEndian.PutUint16(pe[0x98+68:], 10)
	if got := isEFIApplication(pe); got != "" {
		t.Fatalf("a well-formed header was refused: %s", got)
	}
	tests := []struct {
		change func([]byte)
		want   string
	}{
		{func(b []byte) { b[0] = 'X' }, "no MZ header"},
		{func(b []byte) { b[0x81] = 'X' }, "no PE signature"},
		{func(b []byte) { binary.LittleEndian.PutUint32(b[0x3c:], 0x1f0) }, "no PE signature"},
		{func(b []byte) { binary.LittleEndian.PutUint16(b[0x84:], 0x14c) }, "not built for x86_64"},
		{func(b []byte) { binary.LittleEndian.PutUint16(b[0x98:], 0x10b) }, "not PE32+"},
		{func(b []byte) { binary.LittleEndian.PutUint16(b[0x98+68:], 3) }, "not an EFI application"},
	}
	for _, tt := range tests {
		b := append([]byte(nil), pe...)
		tt.change(b)
		if got := isEFIApplication(b); got != tt.want {
			t.Errorf("got %q, want %q", got, tt.want)
		}
	}
	if got := isEFIApplication([]byte("MZ")); got != "no MZ header" {
		t.Errorf("a short file: %q", got)
	}
}
