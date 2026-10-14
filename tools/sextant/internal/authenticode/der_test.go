package authenticode

import (
	"bytes"
	"encoding/asn1"
	"testing"
)

func TestLengthsTakeAsFewBytesAsTheyCan(t *testing.T) {
	cases := []struct {
		n    int
		want []byte
	}{
		{0, []byte{0}},
		{127, []byte{0x7f}},
		{128, []byte{0x81, 0x80}},
		{255, []byte{0x81, 0xff}},
		{256, []byte{0x82, 0x01, 0x00}},
		{70000, []byte{0x83, 0x01, 0x11, 0x70}},
	}
	for _, c := range cases {
		if got := derLength(c.n); !bytes.Equal(got, c.want) {
			t.Errorf("length %d: % x, want % x", c.n, got, c.want)
		}
	}
}

func TestElementsMatchGosOwnEncoder(t *testing.T) {
	long := bytes.Repeat([]byte{7}, 300)
	want, err := asn1.Marshal(long)
	if err != nil {
		t.Fatal(err)
	}
	if got := octets(long); !bytes.Equal(got, want) {
		t.Fatalf("octet string of 300 bytes differs from encoding/asn1")
	}
	type pair struct {
		A int
		B []byte
	}
	want, err = asn1.Marshal(pair{A: 5, B: []byte{1, 2}})
	if err != nil {
		t.Fatal(err)
	}
	if got := seq(small(5), octets([]byte{1, 2})); !bytes.Equal(got, want) {
		t.Fatalf("% x, want % x", got, want)
	}
	if got := tlv(tagSet, small(1), small(2)); !bytes.Equal(got, []byte{0x31, 6, 2, 1, 1, 2, 1, 2}) {
		t.Fatalf("set: % x", got)
	}
}

func TestContentsDropTheTagAndLength(t *testing.T) {
	element := seq(small(1), octets(bytes.Repeat([]byte{1}, 300)))
	got, err := contents(element)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got, element[4:]) {
		t.Fatalf("the contents of a long sequence start after its four header bytes")
	}
	if _, err := contents(append(element, 0)); err == nil {
		t.Fatal("a byte after the element should be refused")
	}
	if _, err := contents([]byte{0x30, 5, 1}); err == nil {
		t.Fatal("a cut element should be refused")
	}
}

func TestObjectIdentifiersAreTheKnownOnes(t *testing.T) {
	cases := map[string][]byte{
		"signedData":       {0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x02},
		"sha256":           {0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01},
		"spcIndirectData":  {0x06, 0x0a, 0x2b, 0x06, 0x01, 0x04, 0x01, 0x82, 0x37, 0x02, 0x01, 0x04},
		"spcPEImageData":   {0x06, 0x0a, 0x2b, 0x06, 0x01, 0x04, 0x01, 0x82, 0x37, 0x02, 0x01, 0x0f},
		"spcSpOpusInfo":    {0x06, 0x0a, 0x2b, 0x06, 0x01, 0x04, 0x01, 0x82, 0x37, 0x02, 0x01, 0x0c},
		"messageDigest":    {0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x04},
		"contentType":      {0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x03},
		"rsaEncryption":    {0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01},
		"statementType":    {0x06, 0x0a, 0x2b, 0x06, 0x01, 0x04, 0x01, 0x82, 0x37, 0x02, 0x01, 0x0b},
		"individualSigner": {0x06, 0x0a, 0x2b, 0x06, 0x01, 0x04, 0x01, 0x82, 0x37, 0x02, 0x01, 0x15},
	}
	got := map[string][]byte{
		"signedData":       oid(oidSignedData),
		"sha256":           oid(oidSHA256),
		"spcIndirectData":  oid(oidSpcIndirectData),
		"spcPEImageData":   oid(oidSpcPEImageData),
		"spcSpOpusInfo":    oid(oidSpcSpOpusInfo),
		"messageDigest":    oid(oidMessageDigest),
		"contentType":      oid(oidContentType),
		"rsaEncryption":    oid(oidRSA),
		"statementType":    oid(oidSpcStatementType),
		"individualSigner": oid(oidIndividualSign),
	}
	for name, want := range cases {
		if !bytes.Equal(got[name], want) {
			t.Errorf("%s: % x, want % x", name, got[name], want)
		}
	}
}
