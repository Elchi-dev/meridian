package stats

import (
	"math"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"testing/fstest"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/srclex"
)

const rustPackage = "[package]\nname = \"x\"\n"

func tree(files map[string]string) fstest.MapFS {
	fsys := fstest.MapFS{}
	for name, body := range files {
		fsys[name] = &fstest.MapFile{Data: []byte(body)}
	}
	return fsys
}

func collect(t *testing.T, files map[string]string) *Report {
	t.Helper()
	r, err := Collect(tree(files))
	if err != nil {
		t.Fatalf("Collect: %v", err)
	}
	return r
}

func component(t *testing.T, r *Report, path string) *Component {
	t.Helper()
	for _, c := range r.Components {
		if c.Path == path {
			return c
		}
	}
	var have []string
	for _, c := range r.Components {
		have = append(have, c.Path)
	}
	t.Fatalf("no component %q, have %v", path, have)
	return nil
}

func file(t *testing.T, c *Component, path string) File {
	t.Helper()
	for _, f := range c.Files {
		if f.Path == path {
			return f
		}
	}
	t.Fatalf("component %s has no file %q", c.Path, path)
	return File{}
}

func TestComponentsAreCratesAndModules(t *testing.T) {
	r := collect(t, map[string]string{
		"Cargo.toml":               "[workspace]\nmembers = [\"kernel\"]\n",
		"kernel/Cargo.toml":        rustPackage,
		"kernel/src/main.rs":       "fn main() {}\n",
		"lib/meri/Cargo.toml":      "  [package]  \nname = \"meri\"\n",
		"lib/meri/src/lib.rs":      "pub fn f() {}\n",
		"tools/sextant/go.mod":     "module x\n",
		"tools/sextant/main.go":    "package main\n",
		"docs/notes.md":            "# not code\n",
		"tests/meri/valid/a.meri":  "a = 1\n",
		"scripts/loose.go":         "package loose\n",
		"lib/meta/Cargo.toml":      "[package.metadata]\n",
		"lib/meta/src/lib.rs":      "fn uncounted() {}\n",
		"tools/sextant/sub/a.go":   "package sub\n",
		"tools/sextant/sub/b.go":   "package sub\n",
		"kernel/src/arch/x86.rs":   "fn a() {}\n",
		"kernel/src/arch/link.ld":  "ENTRY(start)\n",
		"kernel/src/arch/start.s":  ".global start\n",
		"kernel/README.md":         "kernel\n",
		"tools/sextant/sub/c.txt":  "text\n",
		"tools/sextant/sub/d.meri": "d = 1\n",
	})
	var paths []string
	for _, c := range r.Components {
		paths = append(paths, c.Kind+":"+c.Path)
	}
	if got := strings.Join(paths, " "); got != "rust:kernel rust:lib/meri go:tools/sextant" {
		t.Fatalf("components = %s", got)
	}
	k := component(t, r, "kernel")
	if len(k.Files) != 4 || k.Code != 4 {
		t.Errorf("kernel: %d files, %d code lines; want 4 and 4: %+v", len(k.Files), k.Code, k.Files)
	}
	if k.Files[0].Path != "kernel/src/arch/link.ld" || k.Files[3].Path != "kernel/src/main.rs" {
		t.Errorf("files are not sorted by path: %+v", k.Files)
	}
	if file(t, k, "kernel/src/arch/start.s").Lang != srclex.Asm {
		t.Errorf("start.s should count as assembly")
	}
	if s := component(t, r, "tools/sextant"); s.Code != 3 {
		t.Errorf("sextant code lines = %d, want 3", s.Code)
	}
}

func TestSkippedDirectories(t *testing.T) {
	r := collect(t, map[string]string{
		"x/go.mod":                    "module x\n",
		"x/a.go":                      "package x\n",
		"x/target/debug/gen.go":       "package gen\n",
		"x/out/b.go":                  "package out\n",
		"x/testdata/c.go":             "package data\n",
		"x/testdata/inner/go.mod":     "module inner\n",
		"x/node_modules/d.go":         "package nm\n",
		"x/.hidden/e.go":              "package hidden\n",
		".git/hooks/f.go":             "package hooks\n",
		".github/workflows/ci.go":     "package ci\n",
		"x/targets/kept.go":           "package kept\n",
		"x/output/kept.go":            "package kept\n",
		"x/dottedname.v2/kept.go":     "package kept\n",
		"y/.config/go.mod":            "module hidden\n",
		"y/.config/hidden.go":         "package hidden\n",
		"x/testdata.go":               "package x\n",
		"x/node_modules_backup/k.go":  "package k\n",
		"x/sub/target/deeper/skip.go": "package skip\n",
	})
	if len(r.Components) != 1 {
		t.Fatalf("components: %+v", r.Components)
	}
	c := r.Components[0]
	var got []string
	for _, f := range c.Files {
		got = append(got, f.Path)
	}
	want := "x/a.go x/dottedname.v2/kept.go x/node_modules_backup/k.go x/output/kept.go x/targets/kept.go x/testdata.go"
	if strings.Join(got, " ") != want {
		t.Fatalf("counted %v\nwant    %s", got, want)
	}
}

func TestRootComponent(t *testing.T) {
	r := collect(t, map[string]string{
		"go.mod":       "module root\n",
		"main.go":      "package main\n",
		"main_test.go": "package main\n\nfunc TestX() {}\n",
		"tests/x.go":   "package tests\n",
	})
	c := component(t, r, ".")
	if c.Code != 1 || c.Test != 3 {
		t.Fatalf("root component: code %d, test %d; want 1 and 3", c.Code, c.Test)
	}
}

func TestGoTestFiles(t *testing.T) {
	r := collect(t, map[string]string{
		"m/go.mod":      "module m\n",
		"m/a.go":        "package m\n\n// Add adds.\nfunc Add(a, b int) int {\n\treturn a + b\n}\n",
		"m/a_test.go":   "package m\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {\n\t// one plus one\n\tif Add(1, 1) != 2 {\n\t\tt.Fatal()\n\t}\n}\n",
		"m/attest.go":   "package m\n",
		"m/test_x.go":   "package m\n",
		"m/a_test.go.x": "not counted\n",
	})
	c := component(t, r, "m")
	a := file(t, c, "m/a.go")
	if a.Code != 4 || a.Comments != 1 || a.Blank != 1 || a.Test != 0 {
		t.Errorf("a.go: %+v", a)
	}
	at := file(t, c, "m/a_test.go")
	if at.Test != 7 || at.Code != 0 || at.Comments != 1 || at.Blank != 2 {
		t.Errorf("a_test.go: %+v", at)
	}
	if c.Code != 6 || c.Test != 7 {
		t.Errorf("component: code %d, test %d; want 6 and 7", c.Code, c.Test)
	}
}

func rustFile(t *testing.T, src string) File {
	t.Helper()
	r := collect(t, map[string]string{"k/Cargo.toml": rustPackage, "k/src/lib.rs": src})
	return file(t, component(t, r, "k"), "k/src/lib.rs")
}

func TestRustInlineTestModule(t *testing.T) {
	f := rustFile(t, `pub fn double(x: u32) -> u32 {
    x * 2
}

#[cfg(test)]
mod tests {
    use super::*;

    // doubling twice
    #[test]
    fn twice() {
        assert_eq!(double(double(1)), 4);
    }
}
`)
	if f.Code != 3 || f.Test != 8 || f.Comments != 1 || f.Blank != 2 {
		t.Fatalf("got %+v; want code 3, test 8, comments 1, blank 2", f)
	}
}

func TestRustCfgTestOnSingleItems(t *testing.T) {
	f := rustFile(t, `#[cfg(test)]
use std::vec::Vec;
fn real() {}
#[cfg(test)]
fn helper() -> Vec<u8> {
    Vec::new()
}
#[cfg(test)]
struct Pair(u8, u8);
#[cfg(test)]
const TABLE: [u8; 2] = [1, 2];
fn also_real() {}
`)
	if f.Code != 2 || f.Test != 10 {
		t.Fatalf("got %+v; want code 2, test 10", f)
	}
}

func TestRustAttributesAfterCfgTestBelongToTheItem(t *testing.T) {
	f := rustFile(t, `#[cfg(test)]
#[allow(dead_code)]
#[derive(Debug, Clone)]
struct Fixture {
    a: [u8; 4],
}
fn real() {}
`)
	if f.Code != 1 || f.Test != 6 {
		t.Fatalf("got %+v; want code 1, test 6", f)
	}
}

func TestRustCfgVariants(t *testing.T) {
	tests := []struct {
		attr string
		test bool
	}{
		{"#[cfg(test)]", true},
		{"#[cfg(all(test, target_os = \"none\"))]", true},
		{"#[cfg(feature = \"qemu-test\")]", true},
		{"#[cfg(not(test))]", false},
		{"#[cfg(feature = \"other\")]", false},
		{"#[cfg_attr(test, derive(Debug))]", false},
		{"#[test]", false},
		{"#[inline]", false},
	}
	for _, tt := range tests {
		f := rustFile(t, tt.attr+"\nfn item() {}\nfn after() {}\n")
		wantTest := 0
		if tt.test {
			wantTest = 2
		}
		if f.Test != wantTest || f.Code != 3-wantTest {
			t.Errorf("%s: code %d, test %d; want test %d", tt.attr, f.Code, f.Test, wantTest)
		}
	}
}

func TestRustInnerCfgTestMarksTheWholeFile(t *testing.T) {
	f := rustFile(t, "//! helpers\n#![cfg(test)]\nfn a() {}\nfn b() {}\n")
	if f.Code != 0 || f.Test != 3 || f.Comments != 1 {
		t.Fatalf("got %+v", f)
	}
}

func TestRustInnerAttributeThatIsNotCfgTest(t *testing.T) {
	f := rustFile(t, "#![no_std]\n#![allow(dead_code)]\nfn a() {}\n#[cfg(test)]\nfn t() {}\n")
	if f.Code != 3 || f.Test != 2 {
		t.Fatalf("got %+v", f)
	}
}

func TestRustOutOfLineTestModules(t *testing.T) {
	r := collect(t, map[string]string{
		"k/Cargo.toml":               rustPackage,
		"k/src/lib.rs":               "mod parser;\n#[cfg(test)]\nmod tests;\nfn lib() {}\n",
		"k/src/tests.rs":             "fn t1() {}\n",
		"k/src/parser.rs":            "#[cfg(test)]\nmod cases;\nfn parse() {}\n",
		"k/src/parser/cases.rs":      "fn t2() {}\n",
		"k/src/parser/cases/more.rs": "fn t3() {}\n",
		"k/src/parser/real.rs":       "fn real() {}\n",
		"k/src/parser/cases_more.rs": "fn real_too() {}\n",
		"k/src/other/mod.rs":         "#[cfg(test)]\nmod tests;\n",
		"k/src/other/tests/mod.rs":   "fn t4() {}\n",
		"k/src/other/tests/x.rs":     "fn t5() {}\n",
	})
	c := component(t, r, "k")
	for _, p := range []string{"k/src/tests.rs", "k/src/parser/cases.rs", "k/src/parser/cases/more.rs", "k/src/other/tests/mod.rs", "k/src/other/tests/x.rs"} {
		if f := file(t, c, p); f.Test != 1 || f.Code != 0 {
			t.Errorf("%s should be all test: %+v", p, f)
		}
	}
	for _, p := range []string{"k/src/parser/real.rs", "k/src/parser/cases_more.rs"} {
		if f := file(t, c, p); f.Code != 1 || f.Test != 0 {
			t.Errorf("%s should be code: %+v", p, f)
		}
	}
	if lib := file(t, c, "k/src/lib.rs"); lib.Code != 2 || lib.Test != 2 {
		t.Errorf("lib.rs: %+v", lib)
	}
	if c.Code != 5 || c.Test != 11 {
		t.Errorf("component: code %d, test %d; want 5 and 11", c.Code, c.Test)
	}
}

func TestRustIntegrationTestsAndBenches(t *testing.T) {
	r := collect(t, map[string]string{
		"k/Cargo.toml":          rustPackage,
		"k/src/lib.rs":          "fn a() {}\n",
		"k/tests/boot.rs":       "fn t() {}\nfn u() {}\n",
		"k/benches/speed.rs":    "fn b() {}\n",
		"k/src/tests_helper.rs": "fn not_a_test() {}\n",
	})
	c := component(t, r, "k")
	if c.Code != 2 || c.Test != 3 {
		t.Fatalf("code %d, test %d; want 2 and 3", c.Code, c.Test)
	}
}

func TestUnsafeIsCountedOutsideTestsOnly(t *testing.T) {
	f := rustFile(t, `// unsafe in a comment does not count
unsafe fn raw() {}
fn wrap() {
    // SAFETY: nothing to uphold
    unsafe { raw() }
}
const NAME: &str = "unsafe";
#[cfg(test)]
mod tests {
    fn t() { unsafe { super::raw() } }
}
`)
	if f.Unsafe != 2 {
		t.Fatalf("unsafe = %d, want 2", f.Unsafe)
	}
}

func TestFunctionPointerTypesAreNotUnsafeCode(t *testing.T) {
	f := rustFile(t, `pub struct Table {
    pub stall: unsafe extern "efiapi" fn(microseconds: usize) -> Status,
    pub other: unsafe extern /* the default */ "C" fn(),
    pub plain: unsafe fn(u8),
    pub rust: unsafe extern fn(),
}
unsafe extern "C" fn defined() {}
pub unsafe fn also_defined() {}
unsafe impl Sync for Table {}
#[unsafe(no_mangle)]
fn entry() {}
fn call(t: &Table) {
    unsafe { (t.stall)(1) }
}
`)
	// The two functions, the impl, the attribute and the block.
	if f.Unsafe != 5 {
		t.Fatalf("unsafe = %d, want 5", f.Unsafe)
	}
}

func TestAnUnsafeAtTheEndOfAFileIsCounted(t *testing.T) {
	for _, src := range []string{"unsafe", "unsafe extern", "unsafe extern \"C\"", "unsafe fn", "unsafe // fn\n"} {
		if f := rustFile(t, src); f.Unsafe != 1 {
			t.Errorf("%q: unsafe = %d, want 1", src, f.Unsafe)
		}
	}
}

func TestUnsafeInWholeTestFileIsNotCounted(t *testing.T) {
	f := rustFile(t, "#![cfg(test)]\nunsafe fn x() {}\n")
	if f.Unsafe != 0 {
		t.Fatalf("unsafe = %d, want 0", f.Unsafe)
	}
	r := collect(t, map[string]string{
		"k/Cargo.toml":   rustPackage,
		"k/src/lib.rs":   "unsafe fn a() {}\n#[cfg(test)]\nmod tests;\n",
		"k/src/tests.rs": "unsafe fn b() {}\n",
	})
	if c := component(t, r, "k"); c.Unsafe != 1 {
		t.Fatalf("component unsafe = %d, want 1", c.Unsafe)
	}
}

func TestBrokenAttributesDoNotCrash(t *testing.T) {
	for _, src := range []string{
		"fn a() {}\n#[cfg(test)",
		"#[cfg(test)]",
		"#[cfg(test)]\nfn a(",
		"#[cfg(test)]\n#[allow(x)",
		"#[cfg(test)]\nfn a() {",
		"#",
		"#!",
		"#[]\nfn a() {}\n",
	} {
		r, err := Collect(tree(map[string]string{"k/Cargo.toml": rustPackage, "k/src/lib.rs": src}))
		if err != nil {
			t.Errorf("%q: %v", src, err)
			continue
		}
		if len(r.Components) != 1 {
			t.Errorf("%q: %d components", src, len(r.Components))
		}
	}
}

func TestBrokenCfgTestItemCountsToTheEnd(t *testing.T) {
	f := rustFile(t, "fn real() {}\n#[cfg(test)]\nfn open() {\n    let x = 1;\n")
	if f.Code != 1 || f.Test != 3 {
		t.Fatalf("got %+v", f)
	}
}

func TestLexErrorsNameTheFile(t *testing.T) {
	_, err := Collect(tree(map[string]string{"k/Cargo.toml": rustPackage, "k/src/lib.rs": "fn a() {}\n/* open"}))
	if err == nil || err.Error() != "k/src/lib.rs:2:1: block comment is never closed" {
		t.Fatalf("got %v", err)
	}
}

func TestFilesOutsideComponentsAreNotParsed(t *testing.T) {
	r, err := Collect(tree(map[string]string{"loose/broken.rs": "/* never closed", "x/go.mod": "module x\n"}))
	if err != nil {
		t.Fatalf("a file outside every component was parsed: %v", err)
	}
	if len(r.Components) != 1 || len(r.Components[0].Files) != 0 {
		t.Fatalf("got %+v", r.Components)
	}
}

func TestRatioAndMissing(t *testing.T) {
	tests := []struct {
		code, test int
		ratio      float64
		missing    int
	}{
		{0, 0, math.Inf(1), 0},
		{0, 5, math.Inf(1), 0},
		{1, 1, 1.0, 1},
		{1, 2, 2.0, 0},
		{2, 3, 1.5, 0},
		{2, 2, 1.0, 1},
		{3, 4, 4.0 / 3.0, 1},
		{10, 14, 1.4, 1},
		{10, 15, 1.5, 0},
		{10, 30, 3.0, 0},
		{100, 149, 1.49, 1},
		{7, 0, 0, 11},
	}
	for _, tt := range tests {
		c := &Component{Code: tt.code, Test: tt.test}
		if got := c.Ratio(); got != tt.ratio {
			t.Errorf("code %d test %d: ratio %v, want %v", tt.code, tt.test, got, tt.ratio)
		}
		if got := c.Missing(1.5); got != tt.missing {
			t.Errorf("code %d test %d: missing %d, want %d", tt.code, tt.test, got, tt.missing)
		}
	}
}

func TestFailing(t *testing.T) {
	r := &Report{Components: []*Component{
		{Path: "a", Code: 10, Test: 15},
		{Path: "b", Code: 10, Test: 14},
		{Path: "c", Code: 0, Test: 0},
		{Path: "d", Code: 1, Test: 0},
	}}
	var got []string
	for _, c := range r.Failing(1.5) {
		got = append(got, c.Path)
	}
	if strings.Join(got, ",") != "b,d" {
		t.Fatalf("failing = %v, want b,d", got)
	}
	if len(r.Failing(0)) != 0 {
		t.Fatalf("nothing fails a minimum of 0")
	}
}

func TestModuleDir(t *testing.T) {
	tests := map[string]string{
		"k/src/lib.rs":        "k/src",
		"k/src/main.rs":       "k/src",
		"k/src/net/mod.rs":    "k/src/net",
		"k/src/net/tcp.rs":    "k/src/net/tcp",
		"k/src/libfoo.rs":     "k/src/libfoo",
		"k/src/remain.rs":     "k/src/remain",
		"k/src/a/b/mymod.rs":  "k/src/a/b/mymod",
		"k/src/a/b/c/main.rs": "k/src/a/b/c",
	}
	for in, want := range tests {
		if got := moduleDir(in); got != want {
			t.Errorf("moduleDir(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestCollectOnARealDirectory(t *testing.T) {
	root := t.TempDir()
	write := func(rel, body string) {
		p := filepath.Join(root, filepath.FromSlash(rel))
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(p, []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	write("boot/Cargo.toml", rustPackage)
	write("boot/src/main.rs", "fn main() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n")
	write("boot/target/x86_64-unknown-uefi/debug/build/out.rs", "fn generated() {}\n")
	r, err := Collect(os.DirFS(root))
	if err != nil {
		t.Fatal(err)
	}
	c := component(t, r, "boot")
	if c.Code != 1 || c.Test != 5 || len(c.Files) != 1 {
		t.Fatalf("boot: %+v", c)
	}
}

func TestCollectReportsUnreadableTrees(t *testing.T) {
	if _, err := Collect(os.DirFS(filepath.Join(t.TempDir(), "missing"))); err == nil {
		t.Fatal("a missing root should be an error")
	}
}

func TestRustMultiLineAttribute(t *testing.T) {
	f := rustFile(t, "fn real() {}\n#[cfg(\n    test\n)]\nfn t() {}\n")
	if f.Code != 1 || f.Test != 4 {
		t.Fatalf("got %+v; want code 1, test 4", f)
	}
}

func TestRustItemsThatSpanLines(t *testing.T) {
	f := rustFile(t, `#[cfg(test)]
const TABLE: [u8; 2] = [
    1, 2,
];
#[cfg(test)]
fn setup() {
    let a = 1;
    let b = 2;
}
#[cfg(test)]
pub(crate) struct Pair(
    u8,
);
fn real() {}
`)
	if f.Code != 1 || f.Test != 13 {
		t.Fatalf("got %+v; want code 1, test 13", f)
	}
}

func TestBrokenCfgTestItemEndsWithTheFile(t *testing.T) {
	f := rustFile(t, "fn real() {}\n#[cfg(test)]\nfn open() {\n    let x =\n        1")
	if f.Code != 1 || f.Test != 4 {
		t.Fatalf("got %+v; want code 1, test 4", f)
	}
	f = rustFile(t, "#[cfg(test)]\nfn open() {\n    let s = r\"a\nb\"")
	if f.Code != 0 || f.Test != 4 {
		t.Fatalf("multi-line last token: got %+v; want code 0, test 4", f)
	}
}

func TestChildFilesOfInlineTestModules(t *testing.T) {
	r := collect(t, map[string]string{
		"k/Cargo.toml":        rustPackage,
		"k/src/lib.rs":        "#[cfg(test)]\nmod tests {\n    #[cfg(test)]\n    mod deep;\n}\n#[cfg(test)]\nfn t() {\n    mod helper {}\n}\n",
		"k/src/tests/deep.rs": "fn d() {}\n",
		"k/src/deep.rs":       "fn real_deep() {}\n",
		"k/src/helper.rs":     "fn real_helper() {}\n",
	})
	c := component(t, r, "k")
	if f := file(t, c, "k/src/tests/deep.rs"); f.Test != 1 {
		t.Errorf("a child of an inline test module is a test: %+v", f)
	}
	for _, p := range []string{"k/src/deep.rs", "k/src/helper.rs"} {
		if f := file(t, c, p); f.Code != 1 {
			t.Errorf("%s is real code: %+v", p, f)
		}
	}
}

func TestModuleNameOnlyReadsTheHeader(t *testing.T) {
	r := collect(t, map[string]string{
		"k/Cargo.toml":   rustPackage,
		"k/src/lib.rs":   "#[cfg(test)]\npub(crate) mod tests;\n",
		"k/src/tests.rs": "fn t() {}\n",
	})
	if f := file(t, component(t, r, "k"), "k/src/tests.rs"); f.Test != 1 {
		t.Fatalf("pub(crate) mod tests; should make tests.rs a test file: %+v", f)
	}
}

func TestUnsafeIsOnlyARustWord(t *testing.T) {
	r := collect(t, map[string]string{
		"k/Cargo.toml":     rustPackage,
		"k/src/entry.s":    "unsafe:\n    ret\n",
		"k/src/link.ld":    "unsafe = 1;\n",
		"k/src/lib.rs":     "fn a() {}\n",
		"k/src/notes.rs":   "// unsafe\nconst S: &str = \"unsafe\";\n",
		"k/src/really.rs":  "fn f() { unsafe { } }\n",
		"k/src/raw_id.rs":  "fn r#unsafe() {}\n",
		"k/src/lifetim.rs": "fn f<'unsafe>() {}\n",
	})
	if c := component(t, r, "k"); c.Unsafe != 1 {
		t.Fatalf("unsafe = %d, want 1 (only really.rs)", c.Unsafe)
	}
}

func TestStrayHashDoesNotStopTheScan(t *testing.T) {
	f := rustFile(t, "fn a() { let x = 1 #) ; }\n#[cfg(test)]\nfn t() {}\n")
	if f.Code != 1 || f.Test != 2 {
		t.Fatalf("got %+v; want code 1, test 2", f)
	}
}

func TestNestedComponentsOwnTheirFiles(t *testing.T) {
	r := collect(t, map[string]string{
		"Cargo.toml":                    rustPackage,
		"src/main.rs":                   "fn main() {}\n",
		"tools/gen/go.mod":              "module gen\n",
		"tools/gen/gen.go":              "package gen\n\nfunc Gen() {}\n",
		"tools/gen/gen_test.go":         "package gen\n",
		"tools/gen/inner/Cargo.toml":    rustPackage,
		"tools/gen/inner/src/lib.rs":    "pub fn inner() {}\n#[cfg(test)]\nmod t {}\n",
		"tools/gen/inner/build/note.go": "package note\n",
	})
	want := map[string][3]int{
		".":               {1, 0, 1},
		"tools/gen":       {2, 1, 2},
		"tools/gen/inner": {2, 2, 2},
	}
	if len(r.Components) != len(want) {
		t.Fatalf("got %d components, want %d", len(r.Components), len(want))
	}
	for _, c := range r.Components {
		w, ok := want[c.Path]
		if !ok {
			t.Fatalf("unexpected component %q", c.Path)
		}
		if c.Code != w[0] || c.Test != w[1] || len(c.Files) != w[2] {
			t.Errorf("%s: code %d, test %d, files %d; want %v", c.Path, c.Code, c.Test, len(c.Files), w)
		}
	}
	if r.Components[0].Path != "." {
		t.Errorf("the root component sorts first, got %q", r.Components[0].Path)
	}
}

func TestSourcesCarryComponentAndTestLines(t *testing.T) {
	srcs, err := Sources(tree(map[string]string{
		"k/Cargo.toml":   rustPackage,
		"k/src/lib.rs":   "fn a() {}\n#[cfg(test)]\nmod tests {\n    fn t() {}\n}\nfn b() {}\n",
		"k/src/other.rs": "fn real() {}\n",
		"m/go.mod":       "module m\n",
		"m/a.go":         "package m\n",
		"m/a_test.go":    "package m\n",
		"loose/x.go":     "package loose\n",
		"m/notes.md":     "not source\n",
	}))
	if err != nil {
		t.Fatal(err)
	}
	var got []string
	for _, s := range srcs {
		got = append(got, s.Path+"@"+s.Component+":"+s.Kind)
	}
	want := "k/src/lib.rs@k:rust k/src/other.rs@k:rust m/a.go@m:go m/a_test.go@m:go"
	if strings.Join(got, " ") != want {
		t.Fatalf("sources = %v\nwant      %s", got, want)
	}
	lib := srcs[0]
	if lib.AllTest || lib.Lang != srclex.Rust {
		t.Fatalf("lib.rs: %+v", lib)
	}
	for line, want := range map[int]bool{0: false, 1: false, 2: true, 3: true, 5: true, 6: false, 7: false, 99: false} {
		if lib.IsTest(line) != want {
			t.Errorf("lib.rs line %d: IsTest = %v, want %v", line, !want, want)
		}
	}
	if srcs[1].AllTest {
		t.Errorf("other.rs is real code")
	}
	if srcs[2].AllTest || !srcs[3].AllTest || !srcs[3].IsTest(1) {
		t.Errorf("go files: %+v %+v", srcs[2], srcs[3])
	}
}

func TestSourcesAgreeWithCollect(t *testing.T) {
	files := map[string]string{
		"k/Cargo.toml":    rustPackage,
		"k/src/lib.rs":    "fn a() {}\n#[cfg(test)]\nmod tests;\n",
		"k/src/tests.rs":  "fn t() {}\n",
		"k/tests/boot.rs": "fn it() {}\n",
	}
	srcs, err := Sources(tree(files))
	if err != nil {
		t.Fatal(err)
	}
	for _, s := range srcs {
		if s.Path == "k/src/tests.rs" || s.Path == "k/tests/boot.rs" {
			if !s.AllTest {
				t.Errorf("%s should be all test", s.Path)
			}
		}
	}
	if _, err := Sources(tree(map[string]string{"k/Cargo.toml": rustPackage, "k/src/lib.rs": "/* open"})); err == nil {
		t.Fatal("a file that does not lex must be an error")
	}
}

func TestTestRegionsAtTheEdgesOfAFile(t *testing.T) {
	srcs, err := Sources(tree(map[string]string{
		"k/Cargo.toml": rustPackage,
		"k/src/lib.rs": "#[cfg(test)]\nmod tests {}\n",
		"k/src/cut.rs": "fn a() {}\n#[cfg(test)]\nmod",
	}))
	if err != nil {
		t.Fatal(err)
	}
	byPath := map[string]Source{}
	for _, s := range srcs {
		byPath[s.Path] = s
	}
	tests := []struct {
		path string
		want map[int]bool
	}{
		{"k/src/lib.rs", map[int]bool{1: true, 2: true, 3: false}},
		{"k/src/cut.rs", map[int]bool{1: false, 2: true, 3: true, 4: false}},
	}
	for _, tt := range tests {
		s := byPath[tt.path]
		for line, want := range tt.want {
			if s.IsTest(line) != want {
				t.Errorf("%s line %d: IsTest = %v, want %v", tt.path, line, !want, want)
			}
		}
	}
}
