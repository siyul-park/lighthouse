package provider_test

import (
	"strings"
	"testing"

	"github.com/siyul-park/lighthouse/plugins/lang-go/provider"
	"github.com/stretchr/testify/require"
)

const determinismRuns = 20

// frameShaped has what plugins/lang-go/sdk has: two packages that declare the
// same name, an external test package, and a test credited to several symbols.
var frameShaped = map[string]string{
	"go.mod":               testModule,
	"sdk/frame.go":         "package sdk\n\ntype Message struct{ N int }\n\nfunc ReadMessage(n int) Message { return Message{n} }\n\nfunc WriteMessage(m Message) int { return m.N }\n",
	"sdk/frame_test.go":    "package sdk_test\n\nimport (\n\t\"testing\"\n\n\t\"example.com/m/sdk\"\n)\n\nfunc TestWriteMessage(t *testing.T) {\n\tsdk.WriteMessage(sdk.Message{})\n}\n\nfunc TestReadMessage(t *testing.T) {\n\tfor _, tc := range []struct{ n int }{{1}, {2}} {\n\t\tt.Run(\"reads\", func(t *testing.T) { sdk.WriteMessage(sdk.ReadMessage(tc.n)) })\n\t}\n}\n",
	"sdk/internal_test.go": "package sdk\n\nimport \"testing\"\n\nfunc TestMessage(t *testing.T) { _ = ReadMessage(1) }\n",
	"other/read.go":        "package other\n\nfunc ReadMessage() int { return 1 }\n",
	"other/read_test.go":   "package other\n\nimport \"testing\"\n\nfunc TestReadMessage(t *testing.T) { _ = ReadMessage() }\n",
}

func TestProviderDeterminism(t *testing.T) {
	t.Run("answers byte-identically on every run over a frame-shaped package", func(t *testing.T) {
		root := t.TempDir()
		writeTree(t, root, frameShaped)

		first, _ := indexWith(t, provider.New("lang-go", "test"), root, nil)
		for run := 1; run < determinismRuns; run++ {
			again, _ := indexWith(t, provider.New("lang-go", "test"), root, nil)
			require.Equal(t, first, again, "run %d", run)
		}
	})
}

func TestProviderGenericImplementations(t *testing.T) {
	// Slice satisfies cloner by value, Table by pointer, both through a type
	// assertion; Pair satisfies the exported d.I of another package.
	files := map[string]string{
		"go.mod": testModule,
		"d/d.go": "package d\n\ntype I interface {\n\tM() int\n\tN() int\n}\n",
		"p/p.go": "package p\n\nimport \"example.com/m/d\"\n\n" +
			"type cloner interface{ cloneValue() any }\n\n" +
			"type Slice[T any] []T\n\nfunc (s Slice[T]) cloneValue() any { return s }\n\n" +
			"type Table[K comparable] map[K]int\n\nfunc (t *Table[K]) cloneValue() any { return t }\n\n" +
			"type Pair[A any] struct{ a A }\n\nfunc (Pair[A]) M() int { return 1 }\n\nfunc (*Pair[A]) N() int { return 2 }\n\n" +
			"func unused[A any](Pair[A]) {}\n\n" +
			"func Clone(v any) any {\n\tif c, ok := v.(cloner); ok {\n\t\treturn c.cloneValue()\n\t}\n\treturn v\n}\n\n" +
			"var _ d.I = &Pair[int]{}\n",
	}

	t.Run("marks the methods of generic types that satisfy an interface as implementations", func(t *testing.T) {
		root := t.TempDir()
		writeTree(t, root, files)

		got, _ := indexWith(t, provider.New("lang-go", "test"), root, nil)

		require.Equal(t, 4, strings.Count(got, `"implementation":true`))
		require.Contains(t, got, `{"from":{"symbol":"p::Pair#type"},"kind":"implements"`)
	})
}
