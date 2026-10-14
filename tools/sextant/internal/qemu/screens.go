package qemu

import (
	"bytes"
	"errors"
	"fmt"
	"image"
	"image/color"
	"image/png"
	"os"
	"strconv"
	"time"
)

// A test image that wants its screen kept writes "SCREEN <name>" on a line
// of its own and waits for a newline on the serial port. The runner saves
// the screen as <name>.png, when it was asked to keep screens, and sends
// the newline either way.

// screenName reports whether name can be a file name of its own: lower
// case letters, digits and dashes, at most 64 of them.
func screenName(name string) bool {
	if name == "" || len(name) > 64 {
		return false
	}
	for _, r := range name {
		if (r < 'a' || r > 'z') && (r < '0' || r > '9') && r != '-' {
			return false
		}
	}
	return true
}

// readPPM decodes the binary PPM QEMU's screendump writes: "P6", the width
// and height, 255, one whitespace byte, then three bytes per pixel.
func readPPM(data []byte) (*image.RGBA, error) {
	fields := make([]int, 0, 3)
	rest := data
	if !bytes.HasPrefix(rest, []byte("P6")) {
		return nil, errors.New("not a binary PPM file")
	}
	rest = rest[2:]
	for len(fields) < 3 {
		rest = bytes.TrimLeft(rest, " \t\r\n")
		end := bytes.IndexAny(rest, " \t\r\n")
		if end <= 0 {
			return nil, errors.New("the PPM header is cut short")
		}
		n, err := strconv.Atoi(string(rest[:end]))
		if err != nil || n <= 0 || n > 1<<14 {
			return nil, fmt.Errorf("the PPM header holds %q", rest[:end])
		}
		fields = append(fields, n)
		rest = rest[end:]
	}
	width, height, maxval := fields[0], fields[1], fields[2]
	if maxval != 255 {
		return nil, fmt.Errorf("PPM values go up to %d, not 255", maxval)
	}
	pixels := rest[1:]
	if len(pixels) < width*height*3 {
		return nil, fmt.Errorf("the PPM file has %d bytes of pixels, %d short", len(pixels), width*height*3-len(pixels))
	}
	img := image.NewRGBA(image.Rect(0, 0, width, height))
	for i := 0; i < width*height; i++ {
		p := pixels[i*3:]
		img.SetRGBA(i%width, i/width, color.RGBA{R: p[0], G: p[1], B: p[2], A: 255})
	}
	return img, nil
}

// waitForPPM reads path once it holds a whole picture. QEMU writes the file
// while it answers the monitor, so it can be found half written.
func waitForPPM(path string, timeout time.Duration) (*image.RGBA, error) {
	deadline := time.Now().Add(timeout)
	for {
		data, err := os.ReadFile(path)
		if err == nil {
			if img, err := readPPM(data); err == nil {
				return img, nil
			}
		}
		if time.Now().After(deadline) {
			return nil, fmt.Errorf("QEMU wrote no whole screen to %s within %v", path, timeout)
		}
		time.Sleep(20 * time.Millisecond)
	}
}

func writePNG(path string, img image.Image) error {
	var buf bytes.Buffer
	if err := png.Encode(&buf, img); err != nil {
		return err
	}
	return os.WriteFile(path, buf.Bytes(), 0o644)
}
