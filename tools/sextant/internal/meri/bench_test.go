package meri

import (
	"fmt"
	"strings"
	"testing"
)

// benchDocument builds a document that uses every part of the format, about
// a megabyte in size, close to what a large system settings tree looks like.
func benchDocument() []byte {
	var w strings.Builder
	w.WriteString("# A generated document for the parser benchmark.\n\n")
	for i := 0; w.Len() < 1<<20; i++ {
		fmt.Fprintf(&w, "service \"svc-%d\" {\n", i)
		fmt.Fprintf(&w, "    enabled = %t\n", i%3 != 0)
		fmt.Fprintf(&w, "    priority = %d\n", i%32)
		fmt.Fprintf(&w, "    memory = %dMiB    # soft limit\n", 64+i%512)
		fmt.Fprintf(&w, "    timeout = %d.5s\n", i%60)
		fmt.Fprintf(&w, "    weight = 0.%03d\n", i%1000)
		fmt.Fprintf(&w, "    mode = restart-on-failure\n")
		fmt.Fprintf(&w, "    description = \"Service number %d, with \\\"quotes\\\" and \\u{e9}\"\n", i)
		fmt.Fprintf(&w, "    after = [\"svc-%d\", \"net\", disk,]\n", i/2)
		w.WriteString("    exec {\n        path = \"/System/Services/svc\"\n        args = [\n            \"--fast\",  # first\n            \"--log=info\",\n        ]\n    }\n")
		if i%10 == 0 {
			w.WriteString("    motd = \"\"\"\n        Line one of the text.\n          Indented line two.\n        \"\"\"\n")
		}
		w.WriteString("}\n\n")
	}
	return []byte(w.String())
}

func TestBenchmarkDocumentParses(t *testing.T) {
	doc, err := Parse("bench.meri", benchDocument())
	if err != nil {
		t.Fatalf("the benchmark document does not parse: %v", err)
	}
	services := doc.Blocks("service")
	if len(services) < 1000 {
		t.Fatalf("only %d services in the benchmark document", len(services))
	}
	if b, ok := doc.LabelledBlock("service", "svc-10"); !ok {
		t.Fatal("svc-10 is missing")
	} else if s, _ := b.String("motd", ""); s != "Line one of the text.\n  Indented line two." {
		t.Fatalf("motd = %q", s)
	}
}

func BenchmarkParse(b *testing.B) {
	src := benchDocument()
	b.SetBytes(int64(len(src)))
	b.ReportAllocs()
	for b.Loop() {
		if _, err := Parse("bench.meri", src); err != nil {
			b.Fatal(err)
		}
	}
}
