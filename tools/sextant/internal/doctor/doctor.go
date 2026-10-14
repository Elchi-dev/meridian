// Package doctor knows what a machine needs to build, boot and test
// Meridian, how to read the versions it finds, and how each package
// manager installs what is missing.
package doctor

import (
	"slices"
	"strconv"
	"strings"
)

// Manager is a package manager, named by the program that runs it.
type Manager string

const (
	Pacman Manager = "pacman"
	Apt    Manager = "apt-get"
	Dnf    Manager = "dnf"
	Zypper Manager = "zypper"
	Brew   Manager = "brew"
	Winget Manager = "winget"
)

// Managers in the order they are looked for.
var Managers = []Manager{Pacman, Apt, Dnf, Zypper, Brew, Winget}

// Detect is the first package manager lookPath finds, or "" when there is
// none.
func Detect(lookPath func(string) (string, error)) Manager {
	for _, m := range Managers {
		if _, err := lookPath(string(m)); err == nil {
			return m
		}
	}
	return ""
}

// InstallCommands are the command lines that install packages with m:
// one for most managers, one per package for winget, none without
// packages or a known manager.
func InstallCommands(m Manager, packages []string) [][]string {
	if len(packages) == 0 {
		return nil
	}
	with := func(prefix ...string) [][]string {
		return [][]string{append(prefix, packages...)}
	}
	switch m {
	case Pacman:
		return with("sudo", "pacman", "-S", "--needed", "--noconfirm")
	case Apt:
		return with("sudo", "apt-get", "install", "-y")
	case Dnf:
		return with("sudo", "dnf", "install", "-y")
	case Zypper:
		return with("sudo", "zypper", "install", "-y")
	case Brew:
		return with("brew", "install")
	case Winget:
		var out [][]string
		for _, p := range packages {
			out = append(out, []string{"winget", "install", "--exact", "--id", p})
		}
		return out
	}
	return nil
}

// Tool is a program on the PATH that a machine needs or that the tests
// use as a second opinion.
type Tool struct {
	Name    string
	Command string
	Why     string
	// Required tools are needed to build and boot; the others only let
	// the tests check sextant's output with outside programs.
	Required bool
	// Packages names the package that brings the tool, per manager.
	Packages map[Manager]string
}

// Tools are the programs sextant looks for.
var Tools = []Tool{
	{"Go", "go", "builds sextant", true, map[Manager]string{Pacman: "go", Apt: "golang-go", Dnf: "golang", Zypper: "go", Brew: "go", Winget: "GoLang.Go"}},
	{"rustup", "rustup", "installs the pinned Rust toolchain", true, map[Manager]string{Pacman: "rustup", Apt: "rustup", Dnf: "rustup", Zypper: "rustup", Brew: "rustup", Winget: "Rustlang.Rustup"}},
	{"QEMU", "qemu-system-x86_64", "boots the images", true, map[Manager]string{Pacman: "qemu-system-x86", Apt: "qemu-system-x86", Dnf: "qemu-system-x86", Zypper: "qemu-x86", Brew: "qemu", Winget: "SoftwareFreedomConservancy.QEMU"}},
	{"git", "git", "stamps images with the commit", false, map[Manager]string{Pacman: "git", Apt: "git", Dnf: "git", Zypper: "git", Brew: "git", Winget: "Git.Git"}},
	{"OpenSSL", "openssl", "checks the loader's signature", false, map[Manager]string{Pacman: "openssl", Apt: "openssl", Dnf: "openssl", Zypper: "openssl", Brew: "openssl@3"}},
	{"xorriso", "xorriso", "reads the ISO images back", false, map[Manager]string{Pacman: "libisoburn", Apt: "xorriso", Dnf: "xorriso", Zypper: "xorriso", Brew: "xorriso"}},
	{"mtools", "mdir", "reads the FAT partitions back", false, map[Manager]string{Pacman: "mtools", Apt: "mtools", Dnf: "mtools", Zypper: "mtools", Brew: "mtools"}},
	{"fsck.fat", "fsck.fat", "checks the FAT partitions", false, map[Manager]string{Pacman: "dosfstools", Apt: "dosfstools", Dnf: "dosfstools", Zypper: "dosfstools", Brew: "dosfstools"}},
	{"sfdisk", "sfdisk", "reads the partition tables back", false, map[Manager]string{Pacman: "util-linux", Apt: "fdisk", Dnf: "util-linux", Zypper: "util-linux"}},
	{"bsdtar", "bsdtar", "reads the ISO images back a second way", false, map[Manager]string{Pacman: "libarchive", Apt: "libarchive-tools", Dnf: "bsdtar", Zypper: "bsdtar", Brew: "libarchive"}},
	{"qrencode", "qrencode", "checks the QR codes", false, map[Manager]string{Pacman: "qrencode", Apt: "qrencode", Dnf: "qrencode", Zypper: "qrencode", Brew: "qrencode"}},
	{"zbarimg", "zbarimg", "reads the QR codes on the screens", false, map[Manager]string{Pacman: "zbar", Apt: "zbar-tools", Dnf: "zbar", Zypper: "zbar", Brew: "zbar"}},
}

// Firmware names the package that brings OVMF, per manager; Homebrew's
// QEMU and the Windows installer bring their own.
var Firmware = map[Manager]string{Pacman: "edk2-ovmf", Apt: "ovmf", Dnf: "edk2-ovmf", Zypper: "qemu-ovmf-x86_64", Brew: "qemu"}

// Version is a dotted version such as 1.27.3.
type Version [3]int

// ParseVersion reads a version such as 1.27, 1.27.3 or 8.2.2, and the
// number at the start of a part such as 3rc1.
func ParseVersion(s string) (Version, bool) {
	var v Version
	parts := strings.Split(s, ".")
	if len(parts) < 2 || len(parts) > 3 {
		return v, false
	}
	for i, p := range parts {
		digits := len(p) - len(strings.TrimLeft(p, "0123456789"))
		n, err := strconv.Atoi(p[:digits])
		if err != nil {
			return v, false
		}
		v[i] = n
	}
	return v, true
}

// AtLeast says whether v is want or later.
func (v Version) AtLeast(want Version) bool {
	return slices.Compare(v[:], want[:]) >= 0
}

func (v Version) String() string {
	return strconv.Itoa(v[0]) + "." + strconv.Itoa(v[1]) + "." + strconv.Itoa(v[2])
}

// GoVersion reads the version from what `go version` prints, such as
// "go version go1.27.3 linux/amd64".
func GoVersion(output string) (Version, bool) {
	for _, field := range strings.Fields(output) {
		if rest, ok := strings.CutPrefix(field, "go1."); ok {
			return ParseVersion("1." + rest)
		}
	}
	return Version{}, false
}

// QEMUVersion reads the version from the first line `qemu-system-x86_64
// --version` prints, such as "QEMU emulator version 8.2.2 (Debian ...)".
func QEMUVersion(output string) (Version, bool) {
	line, _, _ := strings.Cut(output, "\n")
	_, rest, ok := strings.Cut(line, "version ")
	if !ok {
		return Version{}, false
	}
	field, _, _ := strings.Cut(rest, " ")
	return ParseVersion(field)
}

// Setting is the quoted value of `key = "value"` in a TOML file such as
// rust-toolchain.toml, or of `key value` in go.mod.
func Setting(file, key string) string {
	for _, line := range strings.Split(file, "\n") {
		rest, ok := strings.CutPrefix(strings.TrimSpace(line), key)
		if !ok || rest == "" || (rest[0] != ' ' && rest[0] != '=') {
			continue
		}
		rest = strings.TrimSpace(strings.TrimPrefix(strings.TrimSpace(rest), "="))
		return strings.Trim(rest, `"`)
	}
	return ""
}

// HasToolchain says whether `rustup toolchain list` printed channel,
// whose lines read like "1.99.0-x86_64-unknown-linux-gnu (default)".
func HasToolchain(list, channel string) bool {
	for _, line := range strings.Split(list, "\n") {
		if strings.HasPrefix(strings.TrimSpace(line), channel+"-") {
			return true
		}
	}
	return false
}
