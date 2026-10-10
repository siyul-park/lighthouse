package provider_test

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"

	"github.com/siyul-park/lighthouse/plugins/lang-go/provider"
	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
	"github.com/stretchr/testify/require"
)

const cacheModule = "module example.com/m\n\ngo 1.22\n"

// cacheFixture is a module of five packages: b imports a, e implements the
// interface of d, and c stands alone.
var cacheFixture = map[string]string{
	"go.mod": cacheModule,
	"a/a.go": "package a\n\ntype T struct{ N int }\n\nfunc A() int { return 1 }\n",
	"b/b.go": "package b\n\nimport \"example.com/m/a\"\n\nfunc B() int { return a.A() }\n",
	"c/c.go": "package c\n\nfunc C() int { return 3 }\n",
	"d/d.go": "package d\n\ntype I interface {\n\tM() int\n\tN() int\n}\n",
	"e/e.go": "package e\n\ntype E struct{}\n\nfunc (E) M() int { return 1 }\n\nfunc (E) N() int { return 2 }\n",
}

func TestProviderCache(t *testing.T) {
	t.Run("answers a warm run exactly like a cold one for every conformance case", func(t *testing.T) {
		cases, err := filepath.Glob(casesGlob)
		require.NoError(t, err)
		for _, dir := range cases {
			root, err := filepath.Abs(filepath.Join(dir, "project"))
			require.NoError(t, err)
			for variant, options := range variants(t, dir) {
				name := filepath.Base(dir) + "/" + variant
				cache := t.TempDir()
				want, _ := indexWith(t, provider.New("lang-go", "test"), root, "", options)

				cold, _ := indexWith(t, provider.New("lang-go", "test"), root, cache, options)
				warm, stats := indexWith(t, provider.New("lang-go", "test"), root, cache, options)

				require.JSONEq(t, want, cold, "%s: cold with a cache", name)
				require.JSONEq(t, want, warm, "%s: warm", name)
				if !strings.Contains(want, `"incomplete":[{`) {
					require.Empty(t, stats.Misses, "%s: a warm run analyzes nothing", name)
				}
			}
		}
	})

	t.Run("re-indexes only what an edit can change", func(t *testing.T) {
		root, cache := t.TempDir(), t.TempDir()
		writeTree(t, root, cacheFixture)
		g := provider.New("lang-go", "test")
		steps := []struct {
			name string
			edit func()
			want []string
		}{
			{"cold run", func() {}, []string{"a", "b", "c", "d", "e"}},
			{"nothing changed", func() {}, nil},
			{"a body edit", func() {
				writeFile(t, root, "a/a.go", "package a\n\ntype T struct{ N int }\n\nfunc A() int { return 2 }\n")
			}, []string{"a"}},
			{"a body edit in an implementation", func() {
				writeFile(t, root, "e/e.go", "package e\n\ntype E struct{}\n\nfunc (E) M() int { return 5 }\n\nfunc (E) N() int { return 2 }\n")
			}, []string{"e"}},
			{"a new exported function", func() {
				writeFile(t, root, "a/a.go", "package a\n\ntype T struct{ N int }\n\nfunc A() int { return 2 }\n\nfunc Extra() {}\n")
			}, []string{"a", "b"}},
			{"a file added", func() { writeFile(t, root, "c/c2.go", "package c\n\nfunc C2() {}\n") }, []string{"c"}},
			{"a file removed", func() { require.NoError(t, os.Remove(filepath.Join(root, "c/c2.go"))) }, []string{"c"}},
			{"a type that starts to implement an interface", func() {
				writeFile(t, root, "c/c.go", "package c\n\nfunc C() int { return 3 }\n\ntype X struct{}\n\nfunc (X) M() int { return 0 }\n\nfunc (X) N() int { return 0 }\n")
			}, []string{"c"}},
			{"an interface changed", func() {
				writeFile(t, root, "d/d.go", "package d\n\ntype I interface {\n\tM() int\n\tN() int\n\tO()\n}\n")
			}, []string{"a", "b", "c", "d", "e"}},
			{"go.mod edited", func() { writeFile(t, root, "go.mod", "module example.com/m\n\ngo 1.23\n") }, []string{"a", "b", "c", "d", "e"}},
		}
		for _, step := range steps {
			step.edit()

			got, stats := indexWith(t, g, root, cache, nil)
			want, _ := indexWith(t, provider.New("lang-go", "test"), root, "", nil)

			require.JSONEq(t, want, got, step.name)
			require.Equal(t, step.want, stats.Misses, step.name)
		}
	})

	t.Run("a different option or build tag misses everything", func(t *testing.T) {
		root, cache := t.TempDir(), t.TempDir()
		writeTree(t, root, cacheFixture)
		g := provider.New("lang-go", "test")
		indexWith(t, g, root, cache, nil)

		tagged := map[string]json.RawMessage{"go": json.RawMessage(`{"tags":["x"]}`)}
		got, stats := indexWith(t, g, root, cache, tagged)
		want, _ := indexWith(t, provider.New("lang-go", "test"), root, "", tagged)

		require.JSONEq(t, want, got)
		require.Len(t, stats.Misses, 5)
	})

	t.Run("never caches a unit that did not analyze cleanly", func(t *testing.T) {
		root, cache := t.TempDir(), t.TempDir()
		writeTree(t, root, cacheFixture)
		g := provider.New("lang-go", "test")
		writeFile(t, root, "b/b.go", "package b\n\nfunc B() int { return missing }\n")

		indexWith(t, g, root, cache, nil)
		got, stats := indexWith(t, g, root, cache, nil)
		want, _ := indexWith(t, provider.New("lang-go", "test"), root, "", nil)

		require.JSONEq(t, want, got)
		require.Equal(t, []string{"b"}, stats.Misses, "the broken unit is analyzed again, the rest is not")
	})

	t.Run("runs without a cache when its directory is unusable", func(t *testing.T) {
		root := t.TempDir()
		writeTree(t, root, cacheFixture)
		blocker := filepath.Join(t.TempDir(), "file")
		require.NoError(t, os.WriteFile(blocker, nil, 0o644))

		got, _ := indexWith(t, provider.New("lang-go", "test"), root, filepath.Join(blocker, "cache"), nil)
		want, _ := indexWith(t, provider.New("lang-go", "test"), root, "", nil)

		var a, b sdk.IndexResult
		require.NoError(t, json.Unmarshal([]byte(got), &a))
		require.NoError(t, json.Unmarshal([]byte(want), &b))
		require.Len(t, a.Notices, len(b.Notices)+1, "the notice says why nothing was cached")
		a.Notices = b.Notices
		require.Equal(t, b, a)
	})
}

// indexWith indexes every Go file of root, with the cache directory when it is
// not empty, and returns the result as JSON.
func indexWith(t *testing.T, g *provider.Provider, root, cache string, options map[string]json.RawMessage) (string, provider.RunStats) {
	t.Helper()
	if options == nil {
		options = map[string]json.RawMessage{}
	}
	params := sdk.IndexParams{
		Project:  sdk.ProjectRef{Root: root},
		Language: "go",
		Files:    hashedGoFiles(t, root),
		Context:  sdk.Context{Options: options},
	}
	if cache != "" {
		params.Context.Cache = &sdk.CacheRef{Dir: cache}
	}
	result, err := g.Index(params)
	require.NoError(t, err)
	raw, err := json.Marshal(result)
	require.NoError(t, err)
	stats := g.Stats()
	sort.Strings(stats.Misses)
	return string(raw), stats
}

func hashedGoFiles(t *testing.T, root string) []sdk.FileRef {
	t.Helper()
	files := goFiles(t, root)
	for i := range files {
		text, err := os.ReadFile(filepath.Join(root, files[i].Path))
		require.NoError(t, err)
		sum := sha256.Sum256(text)
		files[i].Hash = hex.EncodeToString(sum[:])
	}
	return files
}

func writeTree(t *testing.T, root string, files map[string]string) {
	t.Helper()
	for name, text := range files {
		if text != "" {
			writeFile(t, root, name, text)
		}
	}
}

func writeFile(t *testing.T, root, name, text string) {
	t.Helper()
	path := filepath.Join(root, filepath.FromSlash(name))
	require.NoError(t, os.MkdirAll(filepath.Dir(path), 0o755))
	require.NoError(t, os.WriteFile(path, []byte(text), 0o644))
}
