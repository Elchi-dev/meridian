// Package secureboot makes what enrolls a key in a firmware's Secure Boot
// databases while the firmware is in Setup Mode: the authenticated
// variables of UEFI 2.10, section 8.2, for db, KEK and PK.
package secureboot

import (
	"encoding/binary"
	"fmt"
	"time"
	"unicode/utf16"

	"github.com/Elchi-dev/meridian/tools/sextant/internal/authenticode"
	"github.com/Elchi-dev/meridian/tools/sextant/internal/disk/gpt"
)

// Attributes of every Secure Boot database: kept across restarts, read at
// boot and at runtime, and written only with a time based signature.
const Attributes = 0x01 | 0x02 | 0x04 | 0x20

var (
	// GlobalVariable is the vendor of PK and KEK.
	GlobalVariable = mustGUID("8BE4DF61-93CA-11D2-AA0D-00E098032B8C")
	// ImageSecurityDatabase is the vendor of db.
	ImageSecurityDatabase = mustGUID("D719B2CB-3D3A-4596-A3BC-DAD00E67656F")
	certX509              = mustGUID("A5C059A1-94E4-4AA7-87B5-AB155C2BF072")
	certTypePKCS7         = mustGUID("4AAFD29D-68DF-49EE-8AA9-347D375665A7")
)

func mustGUID(s string) gpt.GUID {
	g, err := gpt.ParseGUID(s)
	if err != nil {
		panic(err)
	}
	return g
}

// Variable is one of the Secure Boot databases.
type Variable struct {
	Name   string
	Vendor gpt.GUID
}

// Databases in the order they are written: db and KEK while the firmware
// takes anything, then PK, which ends Setup Mode.
var Databases = []Variable{
	{"db", ImageSecurityDatabase},
	{"KEK", GlobalVariable},
	{"PK", GlobalVariable},
}

// SignatureList is an EFI_SIGNATURE_LIST holding one X.509 certificate,
// given in DER, which owner owns.
func SignatureList(owner gpt.GUID, cert []byte) []byte {
	entry := len(owner) + len(cert)
	out := append([]byte(nil), certX509[:]...)
	out = binary.LittleEndian.AppendUint32(out, uint32(28+entry))
	out = binary.LittleEndian.AppendUint32(out, 0)
	out = binary.LittleEndian.AppendUint32(out, uint32(entry))
	out = append(out, owner[:]...)
	return append(out, cert...)
}

// Time is t as an EFI_TIME the way time based authentication takes it:
// in UTC, to the second, every other field zero.
func Time(t time.Time) ([]byte, error) {
	u := t.UTC()
	if u.Year() < 1900 || u.Year() > 9999 {
		return nil, fmt.Errorf("%d is outside the years UEFI counts, 1900 to 9999", u.Year())
	}
	out := binary.LittleEndian.AppendUint16(nil, uint16(u.Year()))
	out = append(out, byte(u.Month()), byte(u.Day()), byte(u.Hour()), byte(u.Minute()), byte(u.Second()))
	return append(out, make([]byte, 9)...), nil
}

// Authenticated is what to write to v to set it to data: an
// EFI_VARIABLE_AUTHENTICATION_2 header made at when, with s's signature
// over v's name and vendor, the attributes, the time and data, then data.
func Authenticated(v Variable, when time.Time, data []byte, s authenticode.Signer) ([]byte, error) {
	stamp, err := Time(when)
	if err != nil {
		return nil, err
	}
	var signed []byte
	for _, unit := range utf16.Encode([]rune(v.Name)) {
		signed = binary.LittleEndian.AppendUint16(signed, unit)
	}
	signed = append(signed, v.Vendor[:]...)
	signed = binary.LittleEndian.AppendUint32(signed, Attributes)
	signed = append(signed, stamp...)
	signed = append(signed, data...)
	signature, err := s.SignDetached(signed)
	if err != nil {
		return nil, err
	}
	out := append([]byte(nil), stamp...)
	// WIN_CERTIFICATE_UEFI_GUID: its length, revision 2.0, the GUID type,
	// the PKCS #7 GUID and the signature.
	out = binary.LittleEndian.AppendUint32(out, uint32(24+len(signature)))
	out = binary.LittleEndian.AppendUint16(out, 0x0200)
	out = binary.LittleEndian.AppendUint16(out, 0x0ef1)
	out = append(out, certTypePKCS7[:]...)
	out = append(out, signature...)
	return append(out, data...), nil
}

// Enroll is what to write to each of the Databases, in their order, so
// that s's certificate becomes the platform key, the key exchange key and
// the one certificate in db. owner owns the certificate in each.
func Enroll(s authenticode.Signer, owner gpt.GUID, when time.Time) ([][]byte, error) {
	if s.Cert == nil {
		return nil, fmt.Errorf("enrolling needs a certificate")
	}
	list := SignatureList(owner, s.Cert.Raw)
	out := make([][]byte, 0, len(Databases))
	for _, v := range Databases {
		data, err := Authenticated(v, when, list, s)
		if err != nil {
			return nil, fmt.Errorf("%s: %w", v.Name, err)
		}
		out = append(out, data)
	}
	return out, nil
}
