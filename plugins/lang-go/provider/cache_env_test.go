package provider_test

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/siyul-park/lighthouse/plugins/lang-go/provider"
	"github.com/stretchr/testify/require"
)

func TestProviderCache_Environment(t *testing.T) {
	t.Run("a go.work member's go.mod edit misses everything", func(t *testing.T) {
		root, cache := t.TempDir(), t.TempDir()
		writeTree(t, root, map[string]string{
			"go.work":      "go 1.22\n\nuse ./m1\n",
			"m1/go.mod":    "module example.com/w\n\ngo 1.22\n",
			"m1/a/a.go":    "package a\n\nfunc A() int { return 1 }\n",
			"m1/b/b.go":    "package b\n\nfunc B() int { return 2 }\n",
			"other/go.mod": "module example.com/o\n\ngo 1.22\n",
		})
		g := provider.New("lang-go", "test")
		indexWith(t, g, root, cache, nil)
		_, warm := indexWith(t, g, root, cache, nil)
		require.Empty(t, warm.Misses)

		writeFile(t, root, "m1/go.mod", "module example.com/w\n\ngo 1.23\n")
		got, stats := indexWith(t, g, root, cache, nil)
		want, _ := indexWith(t, provider.New("lang-go", "test"), root, "", nil)

		require.JSONEq(t, want, got)
		require.Len(t, stats.Misses, 2)
	})

	t.Run("an edit of a replace target, also through a symlink, misses", func(t *testing.T) {
		base := t.TempDir()
		root, cache := filepath.Join(base, "root"), t.TempDir()
		writeTree(t, base, map[string]string{
			"dep/go.mod": "module example.com/dep\n\ngo 1.22\n",
			"dep/d.go":   "package dep\n\nfunc D() int { return 1 }\n",
		})
		require.NoError(t, os.Symlink(filepath.Join(base, "dep"), filepath.Join(base, "link")))
		writeTree(t, root, map[string]string{
			"go.mod": "module example.com/m\n\ngo 1.22\n\nrequire example.com/dep v0.0.0\n\nreplace example.com/dep => ../link\n",
			"a.go":   "package m\n\nimport \"example.com/dep\"\n\nfunc A() int { return dep.D() }\n",
		})
		g := provider.New("lang-go", "test")
		first, _ := indexWith(t, g, root, cache, nil)
		require.NotContains(t, first, `"reason"`, "the project must analyze cleanly")
		_, warm := indexWith(t, g, root, cache, nil)
		require.Empty(t, warm.Misses)
		require.NotEmpty(t, warm.Hits, "the unit must have been cached")

		writeFile(t, base, "dep/d.go", "package dep\n\nfunc D() int { return 1 }\n\nfunc E() {}\n")
		got, stats := indexWith(t, g, root, cache, nil)
		want, _ := indexWith(t, provider.New("lang-go", "test"), root, "", nil)

		require.JSONEq(t, want, got)
		require.Equal(t, []string{"."}, stats.Misses)
	})

	t.Run("a corrupt cache file is a miss", func(t *testing.T) {
		root, cache := t.TempDir(), t.TempDir()
		writeTree(t, root, cacheFixture)
		g := provider.New("lang-go", "test")
		indexWith(t, g, root, cache, nil)
		files, err := filepath.Glob(filepath.Join(cache, "*.json"))
		require.NoError(t, err)
		for _, f := range files {
			require.NoError(t, os.WriteFile(f, []byte("{not json"), 0o644))
		}

		got, stats := indexWith(t, g, root, cache, nil)
		want, _ := indexWith(t, provider.New("lang-go", "test"), root, "", nil)

		require.JSONEq(t, want, got)
		require.Len(t, stats.Misses, 5)
	})

	t.Run("a package that imports C is analyzed again on every run", func(t *testing.T) {
		root, cache := t.TempDir(), t.TempDir()
		writeTree(t, root, cacheFixture)
		writeFile(t, root, "c/c.go", "package c\n\n// #include <stdlib.h>\nimport \"C\"\n\nfunc C1() {}\n")
		g := provider.New("lang-go", "test")

		indexWith(t, g, root, cache, nil)
		got, stats := indexWith(t, g, root, cache, nil)
		want, _ := indexWith(t, provider.New("lang-go", "test"), root, "", nil)

		require.JSONEq(t, want, got)
		require.Equal(t, []string{"c"}, stats.Misses)
	})

	t.Run("a fully cached run on a read-only directory says nothing", func(t *testing.T) {
		root, cache := t.TempDir(), t.TempDir()
		writeTree(t, root, cacheFixture)
		g := provider.New("lang-go", "test")
		indexWith(t, g, root, cache, nil)
		require.NoError(t, os.Chmod(cache, 0o500))
		t.Cleanup(func() { _ = os.Chmod(cache, 0o700) })

		got, stats := indexWith(t, g, root, cache, nil)

		require.Empty(t, stats.Misses)
		require.False(t, strings.Contains(got, "not writable"), got)
	})
}
