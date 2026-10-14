package qemu

import (
	"bytes"
	"image/color"
	"image/png"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"
)

func TestScreenNames(t *testing.T) {
	for _, good := range []string{"status", "panic-1", "a", strings.Repeat("x", 64)} {
		if !screenName(good) {
			t.Errorf("%q should be a name", good)
		}
	}
	for _, bad := range []string{"", "Status", "../etc", "a b", "a.png", "ä", strings.Repeat("x", 65), "a/b"} {
		if screenName(bad) {
			t.Errorf("%q should not be a name", bad)
		}
	}
}

// ppm writes a binary PPM the way QEMU's screendump does.
func ppm(width, height int, pixel func(x, y int) [3]byte) []byte {
	var b bytes.Buffer
	b.WriteString("P6\n")
	b.WriteString(strconv.Itoa(width) + " " + strconv.Itoa(height))
	b.WriteString("\n255\n")
	for y := 0; y < height; y++ {
		for x := 0; x < width; x++ {
			p := pixel(x, y)
			b.Write(p[:])
		}
	}
	return b.Bytes()
}

func TestReadPPM(t *testing.T) {
	data := ppm(3, 2, func(x, y int) [3]byte { return [3]byte{byte(x * 40), byte(y * 90), 7} })
	img, err := readPPM(data)
	if err != nil {
		t.Fatal(err)
	}
	if img.Bounds().Dx() != 3 || img.Bounds().Dy() != 2 {
		t.Fatalf("size %v", img.Bounds())
	}
	if got := img.RGBAAt(2, 1); got != (color.RGBA{R: 80, G: 90, B: 7, A: 255}) {
		t.Fatalf("pixel (2, 1) = %v", got)
	}
	if got := img.RGBAAt(0, 0); got != (color.RGBA{R: 0, G: 0, B: 7, A: 255}) {
		t.Fatalf("pixel (0, 0) = %v", got)
	}
}

func TestReadPPMRefusesBrokenFiles(t *testing.T) {
	good := ppm(2, 2, func(x, y int) [3]byte { return [3]byte{1, 2, 3} })
	tests := map[string][]byte{
		"text PPM":      append([]byte("P3"), good[2:]...),
		"empty":         nil,
		"cut header":    []byte("P6\n2 2"),
		"letters":       []byte("P6\nx 2\n255\n"),
		"zero width":    []byte("P6\n0 2\n255\n"),
		"huge":          []byte("P6\n99999 2\n255\n"),
		"16-bit values": bytes.Replace(good, []byte("255"), []byte("65535"), 1),
		"short pixels":  good[:len(good)-1],
	}
	for name, data := range tests {
		if _, err := readPPM(data); err == nil {
			t.Errorf("%s: no error", name)
		}
	}
	if _, err := readPPM(good); err != nil {
		t.Fatalf("the good file: %v", err)
	}
	if _, err := readPPM(append(append([]byte(nil), good...), 9, 9)); err != nil {
		t.Fatalf("bytes after the pixels are ignored: %v", err)
	}
}

func TestWaitForPPMWaitsForTheWholeFile(t *testing.T) {
	path := filepath.Join(t.TempDir(), "screen.ppm")
	data := ppm(4, 4, func(x, y int) [3]byte { return [3]byte{9, 8, 7} })
	go func() {
		time.Sleep(60 * time.Millisecond)
		_ = os.WriteFile(path, data[:len(data)/2], 0o644)
		time.Sleep(60 * time.Millisecond)
		_ = os.WriteFile(path, data, 0o644)
	}()
	img, err := waitForPPM(path, 5*time.Second)
	if err != nil {
		t.Fatal(err)
	}
	if img.Bounds().Dx() != 4 || img.RGBAAt(3, 3).R != 9 {
		t.Fatalf("got %v", img.Bounds())
	}
}

func TestWaitForPPMGivesUp(t *testing.T) {
	path := filepath.Join(t.TempDir(), "never.ppm")
	start := time.Now()
	if _, err := waitForPPM(path, 100*time.Millisecond); err == nil || !strings.Contains(err.Error(), "no whole screen") {
		t.Fatalf("got %v", err)
	}
	if time.Since(start) > 3*time.Second {
		t.Fatal("waited far too long")
	}
}

func TestWritePNGKeepsThePixels(t *testing.T) {
	img, err := readPPM(ppm(5, 3, func(x, y int) [3]byte { return [3]byte{byte(x), byte(y), 200} }))
	if err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(t.TempDir(), "out.png")
	if err := writePNG(path, img); err != nil {
		t.Fatal(err)
	}
	f, err := os.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	back, err := png.Decode(f)
	if err != nil {
		t.Fatal(err)
	}
	r, g, b, _ := back.At(4, 2).RGBA()
	if r>>8 != 4 || g>>8 != 2 || b>>8 != 200 {
		t.Fatalf("pixel (4, 2) = %d %d %d", r>>8, g>>8, b>>8)
	}
	if err := writePNG(filepath.Join(t.TempDir(), "missing", "out.png"), img); err == nil {
		t.Fatal("writing into a missing folder should fail")
	}
}

func TestScreenLinesAreAnsweredEvenWithoutKeeping(t *testing.T) {
	var answer bytes.Buffer
	w := &scriptWatcher{answer: true, serialIn: &answer, stop: func() {}}
	w.answerPresses("TEST a ok\nSCREEN status\nSCREEN pan")
	if answer.String() != "\n" || len(w.saved) != 0 || w.err != nil {
		t.Fatalf("answer %q, saved %v, err %v", answer.String(), w.saved, w.err)
	}
	w.answerPresses("TEST a ok\nSCREEN status\nSCREEN panic\n")
	if answer.String() != "\n\n" {
		t.Fatalf("the second, now complete line should be answered once: %q", answer.String())
	}
}

func TestABadScreenNameStopsTheRun(t *testing.T) {
	var answer bytes.Buffer
	stopped := false
	w := &scriptWatcher{answer: true, serialIn: &answer, stop: func() { stopped = true }}
	w.answerPresses("SCREEN ../../home\n")
	if w.err == nil || !strings.Contains(w.err.Error(), `named "../../home"`) || !stopped || answer.Len() != 0 {
		t.Fatalf("err %v, stopped %v, answer %q", w.err, stopped, answer.String())
	}
}
