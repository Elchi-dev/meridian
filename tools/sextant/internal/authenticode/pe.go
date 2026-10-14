// Package authenticode signs UEFI executables the way firmware with Secure
// Boot checks them: a PKCS #7 signature over the image's Authenticode
// hash, kept in the image's certificate table. It follows Microsoft's
// "Windows Authenticode Portable Executable Signature Format" and the PE
// format specification.
package authenticode

import (
	"crypto/sha256"
	"encoding/binary"
	"errors"
	"fmt"
	"sort"
)

// span is a range of the file: where it starts and how long it is.
type span struct{ at, size int }

func (s span) end() int { return s.at + s.size }

// layout is what signing needs to know about a PE file.
type layout struct {
	checksumAt int
	// certDirAt is where the certificate table's entry in the data
	// directory is: its file offset, then its size.
	certDirAt int
	headers   int
	sections  []span
	certs     span
}

const (
	pe32     = 0x10b
	pe32Plus = 0x20b
	// certificateTable is the data directory entry of the certificates.
	certificateTable = 4
)

// ErrNotPE is wrapped by every error about a file that is not a PE image
// signing can handle.
var ErrNotPE = errors.New("not a PE image")

func notPE(format string, args ...any) error {
	return fmt.Errorf("%w: %s", ErrNotPE, fmt.Sprintf(format, args...))
}

func u16(b []byte, at int) (int, bool) {
	if at < 0 || at+2 > len(b) {
		return 0, false
	}
	return int(binary.LittleEndian.Uint16(b[at:])), true
}

func u32(b []byte, at int) (int, bool) {
	if at < 0 || at+4 > len(b) {
		return 0, false
	}
	return int(binary.LittleEndian.Uint32(b[at:])), true
}

func parse(b []byte) (layout, error) {
	if len(b) < 64 || b[0] != 'M' || b[1] != 'Z' {
		return layout{}, notPE("no MZ header")
	}
	peAt, _ := u32(b, 0x3c)
	if peAt+24 > len(b) || string(b[peAt:peAt+4]) != "PE\x00\x00" {
		return layout{}, notPE("no PE signature where the MZ header points")
	}
	count, _ := u16(b, peAt+6)
	optSize, _ := u16(b, peAt+20)
	opt := peAt + 24
	magic, ok := u16(b, opt)
	if !ok {
		return layout{}, notPE("the optional header is cut off")
	}
	dirs, countAt := 96, 92
	switch magic {
	case pe32:
	case pe32Plus:
		dirs, countAt = 112, 108
	default:
		return layout{}, notPE("optional header magic %#x", magic)
	}
	entries, _ := u32(b, opt+countAt)
	if entries <= certificateTable || dirs+(certificateTable+1)*8 > optSize {
		return layout{}, notPE("there is no certificate table entry in the data directory")
	}
	l := layout{checksumAt: opt + 64, certDirAt: opt + dirs + certificateTable*8}
	l.headers, _ = u32(b, opt+60)
	certAt, _ := u32(b, l.certDirAt)
	certSize, _ := u32(b, l.certDirAt+4)
	l.certs = span{certAt, certSize}
	table := opt + optSize
	if table+count*40 > len(b) || l.headers > len(b) || l.headers < table+count*40 {
		return layout{}, notPE("the section table does not fit in the headers")
	}
	for i := range count {
		size, _ := u32(b, table+i*40+16)
		at, _ := u32(b, table+i*40+20)
		if size == 0 {
			continue
		}
		if at+size > len(b) || at < l.headers {
			return layout{}, notPE("section %d lies outside the file", i)
		}
		l.sections = append(l.sections, span{at, size})
	}
	sort.Slice(l.sections, func(i, j int) bool { return l.sections[i].at < l.sections[j].at })
	if l.certs.size > 0 && (l.certs.end() != len(b) || l.certs.at < l.headers) {
		return layout{}, notPE("the certificate table is not at the end of the file")
	}
	return l, nil
}

// Digest is the image's Authenticode hash with SHA-256: everything except
// the checksum, the certificate table's directory entry and the
// certificate table itself.
func Digest(image []byte) ([sha256.Size]byte, error) {
	l, err := parse(image)
	if err != nil {
		return [sha256.Size]byte{}, err
	}
	return l.digest(image), nil
}

func (l layout) digest(b []byte) [sha256.Size]byte {
	h := sha256.New()
	h.Write(b[:l.checksumAt])
	h.Write(b[l.checksumAt+4 : l.certDirAt])
	h.Write(b[l.certDirAt+8 : l.headers])
	hashed := l.headers
	for _, s := range l.sections {
		h.Write(b[s.at:s.end()])
		hashed += s.size
	}
	if end := len(b) - l.certs.size; hashed < end {
		h.Write(b[hashed:end])
	}
	var out [sha256.Size]byte
	h.Sum(out[:0])
	return out
}
