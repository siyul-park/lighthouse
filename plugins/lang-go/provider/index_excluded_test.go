package provider_test

import (
	"encoding/json"
	"testing"

	"github.com/siyul-park/lighthouse/plugins/lang-go/provider"
	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
	"github.com/stretchr/testify/require"
)

// excludedTests has foo_arm64_test.go and foo_other_test.go, which exclude
// each other, beside the package they test, with a method and a helper.
var excludedTests = map[string]string{
	"go.mod":            testModule,
	"foo.go":            "package foo\n\ntype Store struct{}\n\nfunc (Store) Get() int { return 1 }\n\nfunc Foo() int { return 1 }\n\nfunc Bar() int { return 2 }\n",
	"foo_arm64_test.go": "package foo\n\nimport \"testing\"\n\nfunc helper() int { return 1 }\n\nfunc TestFoo(t *testing.T) { _ = Foo() }\n\nfunc TestStore_Get(t *testing.T) {}\n\nfunc TestNothing(t *testing.T) {}\n\nfunc ExampleFoo() {}\n\nfunc (Store) TestMethod() {}\n",
	"foo_other_test.go": "//go:build !arm64\n\npackage foo\n\nimport \"testing\"\n\nfunc TestFoo(t *testing.T) { _ = Foo() }\n",
}

func TestIndexExcludedTests(t *testing.T) {
	root := t.TempDir()
	writeTree(t, root, excludedTests)

	t.Run("reads the test entry points of an excluded file from its source", func(t *testing.T) {
		excluded := indexFragments(t, root, "amd64")["foo_arm64_test.go"]

		require.Equal(t, "true", excluded.File.Facts["syntactic"])
		var ids []string
		for _, s := range excluded.Symbols {
			require.Equal(t, "test", s.Kind)
			ids = append(ids, s.ID)
		}
		require.Equal(t, []string{".::TestFoo#test", ".::TestStore_Get#test", ".::TestNothing#test", ".::ExampleFoo#test"}, ids)
		require.Equal(t, []sdk.TestCase{
			{Symbol: ".::TestFoo#test", Style: "scenario", Targets: []string{".::Foo"}},
			{Symbol: ".::TestStore_Get#test", Style: "scenario", Targets: []string{".::Store::Get"}},
			{Symbol: ".::TestNothing#test", Style: "scenario", Targets: []string{}},
		}, excluded.Tests)
		require.Equal(t, []sdk.Module{{Path: ".", Name: "foo"}}, excluded.Modules)
		require.Empty(t, excluded.Edges)
		require.Empty(t, excluded.Functions)
	})

	t.Run("leaves the file the context builds as the type check read it", func(t *testing.T) {
		built := indexFragments(t, root, "amd64")["foo_other_test.go"]

		require.NotContains(t, built.File.Facts, "syntactic")
		require.NotEmpty(t, built.Edges)
	})

	t.Run("marks the other file when the context flips", func(t *testing.T) {
		fragments := indexFragments(t, root, "arm64")

		require.Equal(t, "true", fragments["foo_other_test.go"].File.Facts["syntactic"])
		require.NotContains(t, fragments["foo_arm64_test.go"].File.Facts, "syntactic")
	})
}

func indexFragments(t *testing.T, root, goarch string) map[string]sdk.Fragment {
	t.Helper()
	options, err := json.Marshal(map[string]any{"env": map[string]string{"GOOS": "linux", "GOARCH": goarch}})
	require.NoError(t, err)
	raw, _ := indexWith(t, provider.New("lang-go", "test"), root, map[string]json.RawMessage{"go": options})
	var result answer
	require.NoError(t, json.Unmarshal([]byte(raw), &result))
	found := map[string]sdk.Fragment{}
	for _, f := range result.Fragments {
		found[f.File.Path] = f
	}
	return found
}
