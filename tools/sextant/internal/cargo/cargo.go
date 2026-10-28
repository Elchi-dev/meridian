// Package cargo runs Cargo to build Meridian's parts for their targets.
package cargo

import (
	"context"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
)

// Part is one program Cargo builds for Meridian: its package, the target it
// runs on, and the name of the file Cargo writes.
type Part struct {
	Package string
	Target  string
	File    string
	// Example is the package's example Cargo builds, when the part is one.
	Example string
}

var (
	// Loader is the UEFI bootloader.
	Loader = Part{Package: "boot", Target: "x86_64-unknown-uefi", File: "meridian.efi"}
	// Kernel is the kernel, an ELF file the loader reads.
	Kernel = Part{Package: "kernel", Target: "x86_64-unknown-none", File: "kernel"}
	// Hello is the first program, written on the program runtime.
	Hello = Part{Package: "program", Target: "x86_64-unknown-none", File: "hello", Example: "hello"}
	// Shell reads keys and runs commands on the console.
	Shell = Part{Package: "program", Target: "x86_64-unknown-none", File: "shell", Example: "shell"}
	// Init starts the services and starts each again when it ends.
	Init = Part{Package: "program", Target: "x86_64-unknown-none", File: "init", Example: "init"}
	// Wc counts the lines, words and bytes of files or its standard input.
	Wc = Part{Package: "program", Target: "x86_64-unknown-none", File: "wc", Example: "wc"}
	// Head shows the first lines of files or its standard input.
	Head = Part{Package: "program", Target: "x86_64-unknown-none", File: "head", Example: "head"}
	// Grep shows the lines of files or its standard input that hold a text.
	Grep = Part{Package: "program", Target: "x86_64-unknown-none", File: "grep", Example: "grep"}
	// Sort puts the lines of files or its standard input in order.
	Sort = Part{Package: "program", Target: "x86_64-unknown-none", File: "sort", Example: "sort"}
	// Tail shows the last lines of files or its standard input.
	Tail = Part{Package: "program", Target: "x86_64-unknown-none", File: "tail", Example: "tail"}
	// Uniq shows each run of the same line once.
	Uniq = Part{Package: "program", Target: "x86_64-unknown-none", File: "uniq", Example: "uniq"}
	// Basename shows the last name of a path.
	Basename = Part{Package: "program", Target: "x86_64-unknown-none", File: "basename", Example: "basename"}
	// Dirname shows a path without its last name.
	Dirname = Part{Package: "program", Target: "x86_64-unknown-none", File: "dirname", Example: "dirname"}
	// Tr changes, deletes or squeezes the characters of its standard input.
	Tr = Part{Package: "program", Target: "x86_64-unknown-none", File: "tr", Example: "tr"}
	// Cut shows the bytes, characters or fields of each line a list names.
	Cut = Part{Package: "program", Target: "x86_64-unknown-none", File: "cut", Example: "cut"}
	// Tee writes its standard input to its output and into files.
	Tee = Part{Package: "program", Target: "x86_64-unknown-none", File: "tee", Example: "tee"}
	// Sed runs a script on the lines of its files or its input.
	Sed = Part{Package: "program", Target: "x86_64-unknown-none", File: "sed", Example: "sed"}
	// Expr evaluates the expression its words make.
	Expr = Part{Package: "program", Target: "x86_64-unknown-none", File: "expr", Example: "expr"}
	// Sleep waits as many seconds as it is given.
	Sleep = Part{Package: "program", Target: "x86_64-unknown-none", File: "sleep", Example: "sleep"}
	// Cmp compares two files byte by byte.
	Cmp = Part{Package: "program", Target: "x86_64-unknown-none", File: "cmp", Example: "cmp"}
	// Comm puts the lines of two sorted files in three columns.
	Comm = Part{Package: "program", Target: "x86_64-unknown-none", File: "comm", Example: "comm"}
	// Paste joins the lines of its files side by side.
	Paste = Part{Package: "program", Target: "x86_64-unknown-none", File: "paste", Example: "paste"}
	// Fold breaks lines wider than a width.
	Fold = Part{Package: "program", Target: "x86_64-unknown-none", File: "fold", Example: "fold"}
	// Nl numbers lines.
	Nl = Part{Package: "program", Target: "x86_64-unknown-none", File: "nl", Example: "nl"}
	// Od writes bytes as numbers and characters.
	Od = Part{Package: "program", Target: "x86_64-unknown-none", File: "od", Example: "od"}
	// Join pairs the lines of two files.
	Join = Part{Package: "program", Target: "x86_64-unknown-none", File: "join", Example: "join"}
	// Split cuts a file into files.
	Split = Part{Package: "program", Target: "x86_64-unknown-none", File: "split", Example: "split"}
	// Echo is the shell's echo as a program (decision record 0024).
	Echo = Part{Package: "program", Target: "x86_64-unknown-none", File: "echo", Example: "echo"}
	// Printf is the shell's printf as a program (decision record 0024).
	Printf = Part{Package: "program", Target: "x86_64-unknown-none", File: "printf", Example: "printf"}
	// Test is the shell's test as a program (decision record 0024).
	Test = Part{Package: "program", Target: "x86_64-unknown-none", File: "test", Example: "test"}
	// True is the shell's true as a program (decision record 0024).
	True = Part{Package: "program", Target: "x86_64-unknown-none", File: "true", Example: "true"}
	// False is the shell's false as a program (decision record 0024).
	False = Part{Package: "program", Target: "x86_64-unknown-none", File: "false", Example: "false"}
	// Xargs runs a program on the words of its input.
	Xargs = Part{Package: "program", Target: "x86_64-unknown-none", File: "xargs", Example: "xargs"}
	// Cksum writes POSIX's checksum of files.
	Cksum = Part{Package: "program", Target: "x86_64-unknown-none", File: "cksum", Example: "cksum"}
	// Tsort sorts items in an order pairs of them keep.
	Tsort = Part{Package: "program", Target: "x86_64-unknown-none", File: "tsort", Example: "tsort"}
	// Diff says how two files or two directories of files differ.
	Diff = Part{Package: "program", Target: "x86_64-unknown-none", File: "diff", Example: "diff"}
	// Patch puts what diff wrote into the files it was made from.
	Patch = Part{Package: "program", Target: "x86_64-unknown-none", File: "patch", Example: "patch"}
	// Find walks trees of files and does what its expression says with each.
	Find = Part{Package: "program", Target: "x86_64-unknown-none", File: "find", Example: "find"}
	// Csplit cuts a file into files at the lines its operands name.
	Csplit = Part{Package: "program", Target: "x86_64-unknown-none", File: "csplit", Example: "csplit"}
	// Pathchk says which pathnames could not be used.
	Pathchk = Part{Package: "program", Target: "x86_64-unknown-none", File: "pathchk", Example: "pathchk"}
	// Dd copies a file a block at a time, converting it on the way.
	Dd = Part{Package: "program", Target: "x86_64-unknown-none", File: "dd", Example: "dd"}
	// Rm removes files, and directories with -r.
	Rm = Part{Package: "program", Target: "x86_64-unknown-none", File: "rm", Example: "rm"}
	// Mkdir makes directories, with those above them with -p.
	Mkdir = Part{Package: "program", Target: "x86_64-unknown-none", File: "mkdir", Example: "mkdir"}
	// Rmdir removes empty directories.
	Rmdir = Part{Package: "program", Target: "x86_64-unknown-none", File: "rmdir", Example: "rmdir"}
	// Cat writes files one after another.
	Cat = Part{Package: "program", Target: "x86_64-unknown-none", File: "cat", Example: "cat"}
	// Awk runs awk programs over the records of files.
	Awk = Part{Package: "program", Target: "x86_64-unknown-none", File: "awk", Example: "awk"}
	// Ls writes the names in directories.
	Ls = Part{Package: "program", Target: "x86_64-unknown-none", File: "ls", Example: "ls"}
	// Cp copies files, and directories with -R.
	Cp = Part{Package: "program", Target: "x86_64-unknown-none", File: "cp", Example: "cp"}
	// Mv moves files and directories.
	Mv = Part{Package: "program", Target: "x86_64-unknown-none", File: "mv", Example: "mv"}
	// Strings writes the runs of printable characters in files.
	Strings = Part{Package: "program", Target: "x86_64-unknown-none", File: "strings", Example: "strings"}
	// Du writes how much room the files under paths take.
	Du = Part{Package: "program", Target: "x86_64-unknown-none", File: "du", Example: "du"}
	// Touch makes the files it names that are not there.
	Touch = Part{Package: "program", Target: "x86_64-unknown-none", File: "touch", Example: "touch"}
	// Unlink removes one file.
	Unlink = Part{Package: "program", Target: "x86_64-unknown-none", File: "unlink", Example: "unlink"}
	// Ed edits files with commands.
	Ed = Part{Package: "program", Target: "x86_64-unknown-none", File: "ed", Example: "ed"}
	// Expand turns tabs into spaces.
	Expand = Part{Package: "program", Target: "x86_64-unknown-none", File: "expand", Example: "expand"}
	// Unexpand turns spaces into tabs.
	Unexpand = Part{Package: "program", Target: "x86_64-unknown-none", File: "unexpand", Example: "unexpand"}
	// ExitWith exits with the number it is sent, for the test image's
	// checks of processes.
	ExitWith = Part{Package: "program", Target: "x86_64-unknown-none", File: "exitwith", Example: "exitwith"}
	// WhoAmI sends its own security token back, for the test image's
	// checks of what a program started with less may do.
	WhoAmI = Part{Package: "program", Target: "x86_64-unknown-none", File: "whoami", Example: "whoami"}
	// Threads starts threads and ends with them, for the test image's
	// checks of programs with more than one.
	Threads = Part{Package: "program", Target: "x86_64-unknown-none", File: "threads", Example: "threads"}
	// Arguments sends back the arguments it was started with, for the
	// test image's checks of what a program is handed.
	Arguments = Part{Package: "program", Target: "x86_64-unknown-none", File: "arguments", Example: "arguments"}
	// Programs are what every image carries for the kernel to start.
	Programs = []Part{Hello, Shell, Init, Wc, Head, Grep, Sort, Tail, Uniq, Basename, Dirname, Tr, Cut, Tee, Sed, Expr, Sleep, Cmp, Comm, Paste, Fold, Nl, Od, Expand, Unexpand, Join, Split, Echo, Printf, Test, True, False, Xargs, Cksum, Tsort, Diff, Patch, Find, Csplit, Pathchk, Dd, Rm, Mkdir, Rmdir, Cat, Awk, Ls, Cp, Mv, Strings, Du, Touch, Unlink, Ed}
	// TestPrograms are what test images carry on top.
	TestPrograms = []Part{ExitWith, WhoAmI, Threads, Arguments}
)

// Build is one build of a part.
type Build struct {
	Part
	Root string
	// BuildDir is sextant's build directory. Test images get a folder of
	// their own inside it, so switching between running and testing does
	// not rebuild everything each time.
	BuildDir string
	Release  bool
	QEMUTest bool
}

func (b Build) targetDir() string {
	if b.QEMUTest {
		return filepath.Join(b.BuildDir, "qemu-test")
	}
	return b.BuildDir
}

// Args is the Cargo command line, without "cargo".
func (b Build) Args() []string {
	args := []string{"build", "--locked", "-p", b.Package, "--target", b.Target}
	if b.Example != "" {
		args = append(args, "--example", b.Example)
	}
	if b.Release {
		args = append(args, "--release")
	}
	if b.QEMUTest {
		args = append(args, "--features", "qemu-test")
	}
	return args
}

// Env holds the variables Cargo needs on top of the caller's environment.
func (b Build) Env() []string {
	return []string{"CARGO_TARGET_DIR=" + b.targetDir()}
}

// Output is where Cargo writes the part.
func (b Build) Output() string {
	profile := "debug"
	if b.Release {
		profile = "release"
	}
	dir := filepath.Join(b.targetDir(), b.Target, profile)
	if b.Example != "" {
		dir = filepath.Join(dir, "examples")
	}
	return filepath.Join(dir, b.File)
}

// Run runs Cargo and returns the path of the file it built.
func (b Build) Run(ctx context.Context, stdout, stderr io.Writer) (string, error) {
	cmd := exec.CommandContext(ctx, "cargo", b.Args()...)
	cmd.Dir = b.Root
	cmd.Env = append(os.Environ(), b.Env()...)
	cmd.Stdout, cmd.Stderr = stdout, stderr
	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("cargo %v: %w", b.Args(), err)
	}
	out := b.Output()
	if _, err := os.Stat(out); err != nil {
		return "", fmt.Errorf("cargo finished but %s is missing: %w", out, err)
	}
	return out, nil
}
