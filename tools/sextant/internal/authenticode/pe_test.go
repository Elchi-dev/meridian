package authenticode

import (
	"bytes"
	"crypto/sha256"
	"debug/pe"
	"encoding/binary"
	"errors"
	"testing"
)

const (
	peAt    = 0x80
	headers = 0x400
)

// fakeSection is a section's raw data at a file offset.
type fakeSection struct {
	at   int
	data []byte
}

// buildPE writes a small PE image: the headers, then each section at its
// offset, then extra bytes. plus picks PE32+ over PE32.
func buildPE(plus bool, sections []fakeSection, extra []byte) []byte {
	optSize := 224
	if plus {
		optSize = 240
	}
	size := headers
	for _, s := range sections {
		size = max(size, s.at+len(s.data))
	}
	b := make([]byte, size)
	copy(b, "MZ")
	binary.LittleEndian.PutUint32(b[0x3c:], peAt)
	copy(b[peAt:], "PE\x00\x00")
	binary.LittleEndian.PutUint16(b[peAt+4:], 0x8664)
	binary.LittleEndian.PutUint16(b[peAt+6:], uint16(len(sections)))
	binary.LittleEndian.PutUint16(b[peAt+20:], uint16(optSize))
	opt := peAt + 24
	countAt := 92
	if plus {
		binary.LittleEndian.PutUint16(b[opt:], pe32Plus)
		countAt = 108
	} else {
		binary.LittleEndian.PutUint16(b[opt:], pe32)
	}
	binary.LittleEndian.PutUint32(b[opt+36:], 0x1000)
	binary.LittleEndian.PutUint32(b[opt+40:], 0x200)
	binary.LittleEndian.PutUint32(b[opt+60:], headers)
	binary.LittleEndian.PutUint32(b[opt+64:], 0xdeadbeef)
	binary.LittleEndian.PutUint32(b[opt+countAt:], 16)
	table := opt + optSize
	for i, s := range sections {
		e := b[table+i*40:]
		copy(e, ".s"+string(rune('a'+i)))
		binary.LittleEndian.PutUint32(e[8:], uint32(len(s.data)))
		binary.LittleEndian.PutUint32(e[12:], uint32(0x1000*(i+1)))
		binary.LittleEndian.PutUint32(e[16:], uint32(len(s.data)))
		binary.LittleEndian.PutUint32(e[20:], uint32(s.at))
		copy(b[s.at:], s.data)
	}
	// Bytes in the headers that are hashed, so a wrong range shows.
	for i := table + len(sections)*40; i < headers; i++ {
		b[i] = byte(i)
	}
	return append(b, extra...)
}

func filled(n int, seed byte) []byte {
	b := make([]byte, n)
	for i := range b {
		b[i] = seed + byte(i*7)
	}
	return b
}

// referenceDigest hashes the image from what Go's own PE reader says
// about it, following the Authenticode format document step by step.
func referenceDigest(t *testing.T, b []byte) [sha256.Size]byte {
	t.Helper()
	f, err := pe.NewFile(bytes.NewReader(b))
	if err != nil {
		t.Fatal(err)
	}
	var sizeOfHeaders uint32
	var cert pe.DataDirectory
	switch o := f.OptionalHeader.(type) {
	case *pe.OptionalHeader32:
		sizeOfHeaders, cert = o.SizeOfHeaders, o.DataDirectory[pe.IMAGE_DIRECTORY_ENTRY_SECURITY]
	case *pe.OptionalHeader64:
		sizeOfHeaders, cert = o.SizeOfHeaders, o.DataDirectory[pe.IMAGE_DIRECTORY_ENTRY_SECURITY]
	}
	opt := int(binary.LittleEndian.Uint32(b[0x3c:])) + 24
	checksum := opt + 64
	dirs := opt + 96
	if _, ok := f.OptionalHeader.(*pe.OptionalHeader64); ok {
		dirs = opt + 112
	}
	certEntry := dirs + 4*8
	h := sha256.New()
	h.Write(b[:checksum])
	h.Write(b[checksum+4 : certEntry])
	h.Write(b[certEntry+8 : sizeOfHeaders])
	sum := int(sizeOfHeaders)
	type raw struct{ off, size uint32 }
	var raws []raw
	for _, s := range f.Sections {
		if s.Size > 0 {
			raws = append(raws, raw{s.Offset, s.Size})
		}
	}
	for i := range raws {
		for j := i + 1; j < len(raws); j++ {
			if raws[j].off < raws[i].off {
				raws[i], raws[j] = raws[j], raws[i]
			}
		}
	}
	for _, r := range raws {
		h.Write(b[r.off : r.off+r.size])
		sum += int(r.size)
	}
	if end := len(b) - int(cert.Size); sum < end {
		h.Write(b[sum:end])
	}
	var out [sha256.Size]byte
	h.Sum(out[:0])
	return out
}

func TestTheDigestFollowsTheFormatDocument(t *testing.T) {
	images := map[string][]byte{
		"PE32+ with two sections": buildPE(true, []fakeSection{{0x400, filled(0x200, 1)}, {0x600, filled(0x400, 2)}}, nil),
		"PE32":                    buildPE(false, []fakeSection{{0x400, filled(0x200, 3)}}, nil),
		"sections out of order":   buildPE(true, []fakeSection{{0x800, filled(0x200, 4)}, {0x400, filled(0x400, 5)}}, nil),
		"extra data at the end":   buildPE(true, []fakeSection{{0x400, filled(0x200, 6)}}, filled(100, 7)),
		"no sections":             buildPE(true, nil, filled(33, 8)),
		"a section without data":  buildPE(true, []fakeSection{{0, nil}, {0x400, filled(0x200, 9)}}, nil),
	}
	for name, b := range images {
		got, err := Digest(b)
		if err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		if want := referenceDigest(t, b); got != want {
			t.Errorf("%s: digest %x, the reference says %x", name, got, want)
		}
	}
}

func TestTheDigestIgnoresOnlyWhatItShould(t *testing.T) {
	b := buildPE(true, []fakeSection{{0x400, filled(0x200, 1)}}, filled(16, 9))
	base, _ := Digest(b)
	opt := peAt + 24
	// The entry's offset, while its size stays 0: a different size would
	// change which bytes are the certificate table.
	ignored := []int{opt + 64, opt + 67, opt + 112 + 32, opt + 112 + 35}
	for _, at := range ignored {
		c := bytes.Clone(b)
		c[at] ^= 0xff
		if d, err := Digest(c); err != nil || d != base {
			t.Errorf("byte %#x is the checksum or the certificate entry and should not count: %v", at, err)
		}
	}
	counted := []int{0x10, opt + 63, opt + 68, opt + 112 + 31, opt + 112 + 40, headers - 1, headers, len(b) - 1}
	for _, at := range counted {
		c := bytes.Clone(b)
		c[at] ^= 0xff
		if d, _ := Digest(c); d == base {
			t.Errorf("byte %#x should count", at)
		}
	}
}

func TestBrokenImagesAreRefused(t *testing.T) {
	good := buildPE(true, []fakeSection{{0x400, filled(0x200, 1)}}, nil)
	opt := peAt + 24
	cases := map[string]func([]byte) []byte{
		"too short":           func(b []byte) []byte { return b[:63] },
		"no MZ":               func(b []byte) []byte { b[0] = 'X'; return b },
		"MX":                  func(b []byte) []byte { b[1] = 'X'; return b },
		"no PE signature":     func(b []byte) []byte { b[peAt+2] = 1; return b },
		"PE header past end":  func(b []byte) []byte { binary.LittleEndian.PutUint32(b[0x3c:], uint32(len(b))); return b },
		"unknown magic":       func(b []byte) []byte { b[opt] = 0x07; return b },
		"too few directories": func(b []byte) []byte { binary.LittleEndian.PutUint32(b[opt+108:], 4); return b },
		"optional header cut": func(b []byte) []byte { binary.LittleEndian.PutUint16(b[peAt+20:], 0xffff); return b },
		"short optional header": func(b []byte) []byte {
			binary.LittleEndian.PutUint16(b[peAt+20:], 112+4*8)
			return b
		},
		"section past the end": func(b []byte) []byte {
			binary.LittleEndian.PutUint32(b[opt+240+16:], 0x10000)
			return b
		},
		"section in the headers": func(b []byte) []byte {
			binary.LittleEndian.PutUint32(b[opt+240+20:], 0x200)
			return b
		},
		"headers past the end": func(b []byte) []byte {
			binary.LittleEndian.PutUint32(b[opt+60:], uint32(len(b)+1))
			return b
		},
		"section table past the headers": func(b []byte) []byte {
			binary.LittleEndian.PutUint32(b[opt+60:], uint32(opt+240+20))
			return b
		},
		"certificates over the headers": func(b []byte) []byte {
			binary.LittleEndian.PutUint32(b[opt+144:], 0x10)
			binary.LittleEndian.PutUint32(b[opt+148:], uint32(len(b)-0x10))
			return b
		},
		"certificates not at the end": func(b []byte) []byte {
			binary.LittleEndian.PutUint32(b[opt+144:], 0x400)
			binary.LittleEndian.PutUint32(b[opt+148:], 8)
			return b
		},
	}
	for name, breakIt := range cases {
		if _, err := Digest(breakIt(bytes.Clone(good))); !errors.Is(err, ErrNotPE) {
			t.Errorf("%s: got %v", name, err)
		}
	}
	if _, err := Digest(good); err != nil {
		t.Fatalf("the good image: %v", err)
	}
	bare := buildPE(true, nil, nil)
	binary.LittleEndian.PutUint32(bare[opt+60:], uint32(len(bare)+8))
	if _, err := Digest(bare); !errors.Is(err, ErrNotPE) {
		t.Errorf("headers past the end without sections: %v", err)
	}
	pe32 := buildPE(false, nil, nil)
	binary.LittleEndian.PutUint32(pe32[opt+92:], 4)
	if _, err := Digest(pe32); !errors.Is(err, ErrNotPE) {
		t.Errorf("PE32 with too few directories: %v", err)
	}
}
