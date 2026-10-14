package authenticode

import (
	"encoding/asn1"
	"fmt"
)

// DER tags used in the signature. Context tags are written out in full,
// as the byte that starts the element.
const (
	tagInteger     = 0x02
	tagBitString   = 0x03
	tagOctetString = 0x04
	tagNull        = 0x05
	tagOID         = 0x06
	tagSequence    = 0x30
	tagSet         = 0x31
	// explicit0 is [0] wrapping another element; implicit0 is [0] in
	// place of a SET or SEQUENCE.
	explicit0 = 0xa0
	explicit2 = 0xa2
	implicit0 = 0xa0
	// implicitBMP0 is [0] in place of a BMPString, a primitive type.
	implicitBMP0 = 0x80
)

// tlv encodes one DER element: its tag, its length and its contents, the
// parts written one after the other.
func tlv(tag byte, parts ...[]byte) []byte {
	n := 0
	for _, p := range parts {
		n += len(p)
	}
	out := append([]byte{tag}, derLength(n)...)
	for _, p := range parts {
		out = append(out, p...)
	}
	return out
}

// derLength is a DER length: one byte below 128, otherwise a byte that
// says how many bytes follow and then the length, most significant first.
func derLength(n int) []byte {
	if n < 0x80 {
		return []byte{byte(n)}
	}
	var digits []byte
	for v := n; v > 0; v >>= 8 {
		digits = append([]byte{byte(v)}, digits...)
	}
	return append([]byte{0x80 | byte(len(digits))}, digits...)
}

func seq(parts ...[]byte) []byte { return tlv(tagSequence, parts...) }

func oid(id asn1.ObjectIdentifier) []byte {
	b, err := asn1.Marshal(id)
	if err != nil {
		panic(fmt.Sprintf("the object identifier %v cannot be encoded: %v", id, err))
	}
	return b
}

func octets(b []byte) []byte { return tlv(tagOctetString, b) }

func small(n int) []byte {
	b, err := asn1.Marshal(n)
	if err != nil {
		panic(err)
	}
	return b
}

// contents is what follows the tag and the length of one DER element.
func contents(element []byte) ([]byte, error) {
	var raw asn1.RawValue
	rest, err := asn1.Unmarshal(element, &raw)
	if err != nil {
		return nil, err
	}
	if len(rest) != 0 {
		return nil, fmt.Errorf("%d bytes after the element", len(rest))
	}
	return raw.Bytes, nil
}

// Object identifiers from PKCS #7, PKCS #9 and Authenticode.
var (
	oidSignedData       = asn1.ObjectIdentifier{1, 2, 840, 113549, 1, 7, 2}
	oidSHA256           = asn1.ObjectIdentifier{2, 16, 840, 1, 101, 3, 4, 2, 1}
	oidRSA              = asn1.ObjectIdentifier{1, 2, 840, 113549, 1, 1, 1}
	oidContentType      = asn1.ObjectIdentifier{1, 2, 840, 113549, 1, 9, 3}
	oidMessageDigest    = asn1.ObjectIdentifier{1, 2, 840, 113549, 1, 9, 4}
	oidSpcIndirectData  = asn1.ObjectIdentifier{1, 3, 6, 1, 4, 1, 311, 2, 1, 4}
	oidSpcPEImageData   = asn1.ObjectIdentifier{1, 3, 6, 1, 4, 1, 311, 2, 1, 15}
	oidSpcSpOpusInfo    = asn1.ObjectIdentifier{1, 3, 6, 1, 4, 1, 311, 2, 1, 12}
	oidSpcStatementType = asn1.ObjectIdentifier{1, 3, 6, 1, 4, 1, 311, 2, 1, 11}
	oidIndividualSign   = asn1.ObjectIdentifier{1, 3, 6, 1, 4, 1, 311, 2, 1, 21}
)
