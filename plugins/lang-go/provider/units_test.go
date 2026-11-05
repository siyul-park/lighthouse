package provider_test

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"io/fs"
	"os"
	"path"
	"path/filepath"
	"sort"
	"strings"
	"testing"

	"github.com/siyul-park/lighthouse/plugins/lang-go/provider"
	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
	"github.com/stretchr/testify/require"
)

// unitFixture is a module of five packages: b imports a, e implements the
// interface of d, and c stands alone.
var unitFixture = map[string]string{
	"go.mod": testModule,
	"a/a.go": "package a\n\ntype T struct{ N int }\n\nfunc A() int { return 1 }\n",
	"b/b.go": "package b\n\nimport \"example.com/m/a\"\n\nfunc B() int { return a.A() }\n",
	"c/c.go": "package c\n\nfunc C() int { return 3 }\n",
	"d/d.go": "package d\n\ntype I interface {\n\tM() int\n\tN() int\n}\n",
	"e/e.go": "package e\n\ntype E struct{}\n\nfunc (E) M() int { return 1 }\n\nfunc (E) N() int { return 2 }\n",
}

func byID(units []sdk.UnitResult) map[string]sdk.UnitResult {
	out := map[string]sdk.UnitResult{}
	for _, unit := range units {
		out[unit.ID] = unit
	}
	return out
}

func TestProviderUnits(t *testing.T) {
	t.Run("a unit per directory, with the api and the imports of its package", func(t *testing.T) {
		root := t.TempDir()
		writeTree(t, root, unitFixture)

		units := byID(unitsAt(t, root))

		require.Len(t, units, 5)
		require.Equal(t, []string{"a", "d"}, units["b"].Imports, "b imports a and the package that declares an interface")
		require.Empty(t, units["d"].Imports)
		require.NotEmpty(t, units["a"].Api)
		require.NotEmpty(t, units["a"].Body)
	})

	t.Run("a body edit moves the body of its unit and no api", func(t *testing.T) {
		root := t.TempDir()
		writeTree(t, root, unitFixture)
		before := byID(unitsAt(t, root))

		writeFile(t, root, "a/a.go", "package a\n\ntype T struct{ N int }\n\nfunc A() int { return 2 }\n")
		after := byID(unitsAt(t, root))

		require.NotEqual(t, before["a"].Body, after["a"].Body)
		for id := range before {
			require.Equal(t, before[id].Api, after[id].Api, id)
		}
	})

	t.Run("a comment or a change of layout moves the body of its unit and no api", func(t *testing.T) {
		root := t.TempDir()
		writeTree(t, root, unitFixture)
		before := byID(unitsAt(t, root))

		writeFile(t, root, "a/a.go", "// Package a.\npackage a\n\n// T is a type.\ntype T struct{ N int } // the field\n\n\n// A answers.\nfunc A() int {\n\treturn 1 // one\n}\n")
		after := byID(unitsAt(t, root))

		require.NotEqual(t, before["a"].Body, after["a"].Body)
		for id := range before {
			require.Equal(t, before[id].Api, after[id].Api, id)
		}
	})

	t.Run("a build directive moves the api of its package", func(t *testing.T) {
		root := t.TempDir()
		writeTree(t, root, unitFixture)
		before := byID(unitsAt(t, root))

		writeFile(t, root, "c/c.go", "//go:build linux\n\npackage c\n\nfunc C() int { return 3 }\n")
		after := byID(unitsAt(t, root))

		require.NotEqual(t, before["c"].Api, after["c"].Api)
	})

	t.Run("a new exported function moves the api of its package and its importers", func(t *testing.T) {
		root := t.TempDir()
		writeTree(t, root, unitFixture)
		before := byID(unitsAt(t, root))

		writeFile(t, root, "a/a.go", "package a\n\ntype T struct{ N int }\n\nfunc A() int { return 1 }\n\nfunc Extra() {}\n")
		after := byID(unitsAt(t, root))

		require.NotEqual(t, before["a"].Api, after["a"].Api)
		require.NotEqual(t, before["b"].Api, after["b"].Api)
		require.Equal(t, before["c"].Api, after["c"].Api)
		require.Equal(t, before["d"].Api, after["d"].Api)
	})

	t.Run("an interface that changed moves the api of the package that declares it", func(t *testing.T) {
		root := t.TempDir()
		writeTree(t, root, unitFixture)
		before := byID(unitsAt(t, root))

		writeFile(t, root, "d/d.go", "package d\n\ntype I interface {\n\tM() int\n\tN() int\n\tO()\n}\n")
		after := byID(unitsAt(t, root))

		require.NotEqual(t, before["d"].Api, after["d"].Api)
		require.Equal(t, before["c"].Api, after["c"].Api)
	})

	t.Run("a replace target outside the project is read, and its edit moves the api of the importer", func(t *testing.T) {
		base := t.TempDir()
		root := filepath.Join(base, "root")
		writeTree(t, base, map[string]string{
			"dep/go.mod": "module example.com/dep\n\ngo 1.22\n",
			"dep/d.go":   "package dep\n\nfunc D() int { return 1 }\n",
		})
		require.NoError(t, os.Symlink(filepath.Join(base, "dep"), filepath.Join(base, "link")))
		writeTree(t, root, map[string]string{
			"go.mod": "module example.com/m\n\ngo 1.22\n\nrequire example.com/dep v0.0.0\n\nreplace example.com/dep => ../link\n",
			"a.go":   "package m\n\nimport \"example.com/dep\"\n\nfunc A() int { return dep.D() }\n",
		})
		before := byID(unitsAt(t, root))
		resolved, err := filepath.EvalSymlinks(filepath.Join(base, "dep", "d.go"))
		require.NoError(t, err)
		require.Contains(t, before["."].Reads, resolved)

		writeFile(t, base, "dep/d.go", "package dep\n\nfunc D() int { return 1 }\n\nfunc E() {}\n")
		after := byID(unitsAt(t, root))

		require.NotEqual(t, before["."].Api, after["."].Api)
	})

	t.Run("answers the same whether the directories are asked together or alone", func(t *testing.T) {
		root := t.TempDir()
		writeTree(t, root, unitFixture)
		together := unitsAt(t, root)

		var alone []sdk.UnitResult
		for _, unit := range unitRefs(t, root) {
			result, err := provider.New("lang-go", "test").Index(sdk.IndexParams{
				Project:  sdk.ProjectRef{Root: root},
				Language: "go",
				Units:    []sdk.UnitRef{unit},
				Files:    projectFiles(t, root),
			})
			require.NoError(t, err)
			alone = append(alone, result.Units...)
		}

		require.Equal(t, together, alone)
	})

	t.Run("loads the packages whose interfaces the types of the unit can implement", func(t *testing.T) {
		root := t.TempDir()
		writeTree(t, root, unitFixture)
		packages := map[string]int{}
		for _, unit := range unitRefs(t, root) {
			g := provider.New("lang-go", "test")
			_, err := g.Index(sdk.IndexParams{
				Project:  sdk.ProjectRef{Root: root},
				Language: "go",
				Units:    []sdk.UnitRef{unit},
				Files:    projectFiles(t, root),
			})
			require.NoError(t, err)
			packages[unit.ID] = g.Stats().Packages
		}

		// e has the methods of the interface of d; no other package has any
		// method, so none of them needs d to be loaded with it.
		require.Equal(t, map[string]int{"a": 1, "b": 1, "c": 1, "d": 1, "e": 2}, packages)
	})

	t.Run("loads every interface package for a type that embeds another", func(t *testing.T) {
		root := t.TempDir()
		writeTree(t, root, unitFixture)
		writeFile(t, root, "c/c.go", "package c\n\nimport \"example.com/m/e\"\n\ntype C struct{ e.E }\n")
		g := provider.New("lang-go", "test")
		var unit sdk.UnitRef
		for _, ref := range unitRefs(t, root) {
			if ref.ID == "c" {
				unit = ref
			}
		}

		_, err := g.Index(sdk.IndexParams{
			Project:  sdk.ProjectRef{Root: root},
			Language: "go",
			Units:    []sdk.UnitRef{unit},
			Files:    projectFiles(t, root),
		})

		require.NoError(t, err)
		require.Equal(t, 2, g.Stats().Packages, "c and d, whose interface the methods it takes from e may implement")
	})

	t.Run("a failure of the go command is a gap of a unit, so its answer can be reused", func(t *testing.T) {
		root := t.TempDir()
		writeTree(t, root, unitFixture)
		options, err := json.Marshal(map[string]any{"env": map[string]string{"GOOS": "linux", "GOARCH": "no-such-arch"}})
		require.NoError(t, err)

		result, err := provider.New("lang-go", "test").Index(sdk.IndexParams{
			Project:  sdk.ProjectRef{Root: root},
			Language: "go",
			Units:    unitRefs(t, root),
			Files:    projectFiles(t, root),
			Context:  sdk.Context{Options: map[string]json.RawMessage{"go": options}},
		})

		require.NoError(t, err)
		require.Empty(t, result.Incomplete, "a gap of no unit is an answer the host cannot keep")
		gaps := 0
		for _, unit := range result.Units {
			for _, gap := range unit.Incomplete {
				if strings.HasPrefix(gap.Reason, "go list failed in") {
					gaps++
					require.NotEmpty(t, gap.Path)
				}
			}
		}
		require.Equal(t, 1, gaps)
	})
}

// unitsAt indexes every directory of the project as a unit of its own.
func unitsAt(t *testing.T, root string) []sdk.UnitResult {
	t.Helper()
	result, err := provider.New("lang-go", "test").Index(sdk.IndexParams{
		Project:  sdk.ProjectRef{Root: root},
		Language: "go",
		Units:    unitRefs(t, root),
		Files:    projectFiles(t, root),
	})
	require.NoError(t, err)
	require.Empty(t, result.Incomplete)
	return result.Units
}

// unitRefs groups the files of the project by directory, as the host does.
func unitRefs(t *testing.T, root string) []sdk.UnitRef {
	t.Helper()
	groups := map[string][]sdk.FileRef{}
	for _, rel := range projectFiles(t, root) {
		data, err := os.ReadFile(filepath.Join(root, filepath.FromSlash(rel)))
		require.NoError(t, err)
		sum := sha256.Sum256(data)
		id := path.Dir(rel)
		groups[id] = append(groups[id], sdk.FileRef{Path: rel, Hash: hex.EncodeToString(sum[:])})
	}
	ids := make([]string, 0, len(groups))
	for id := range groups {
		ids = append(ids, id)
	}
	sort.Strings(ids)
	units := make([]sdk.UnitRef, 0, len(ids))
	for _, id := range ids {
		units = append(units, sdk.UnitRef{ID: id, Files: groups[id]})
	}
	return units
}

// projectFiles are the Go files of the project, as the host lists them.
func projectFiles(t *testing.T, root string) []string {
	t.Helper()
	var files []string
	require.NoError(t, filepath.WalkDir(root, func(p string, entry fs.DirEntry, err error) error {
		if err != nil || entry.IsDir() || !strings.HasSuffix(p, ".go") {
			return err
		}
		rel, err := filepath.Rel(root, p)
		files = append(files, filepath.ToSlash(rel))
		return err
	}))
	sort.Strings(files)
	return files
}

func writeTree(t *testing.T, root string, files map[string]string) {
	t.Helper()
	for name, text := range files {
		writeFile(t, root, name, text)
	}
}

func writeFile(t *testing.T, root, name, text string) {
	t.Helper()
	target := filepath.Join(root, filepath.FromSlash(name))
	require.NoError(t, os.MkdirAll(filepath.Dir(target), 0o755))
	require.NoError(t, os.WriteFile(target, []byte(text), 0o644))
}
