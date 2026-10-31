package provider_test

import (
	"testing"

	"github.com/siyul-park/lighthouse/plugins/lang-go/provider"
	"github.com/stretchr/testify/require"
)

const determinismRuns = 20

// frameShaped has what plugins/lang-go/sdk has: two packages that declare the
// same name, an external test package, and a test credited to several symbols.
var frameShaped = map[string]string{
	"go.mod":               cacheModule,
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

		first, _ := indexWith(t, provider.New("lang-go", "test"), root, "", nil)
		for run := 1; run < determinismRuns; run++ {
			again, _ := indexWith(t, provider.New("lang-go", "test"), root, "", nil)
			require.Equal(t, first, again, "run %d", run)
		}
	})

	t.Run("answers a cached run exactly like a cold one, run after run", func(t *testing.T) {
		root := t.TempDir()
		writeTree(t, root, frameShaped)
		cache := t.TempDir()

		first, _ := indexWith(t, provider.New("lang-go", "test"), root, "", nil)
		for run := 0; run < determinismRuns; run++ {
			again, _ := indexWith(t, provider.New("lang-go", "test"), root, cache, nil)
			require.Equal(t, first, again, "run %d", run)
		}
	})
}
