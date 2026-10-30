package provider_test

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/siyul-park/lighthouse/plugins/lang-go/provider"
	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
	"github.com/stretchr/testify/require"
)

func TestSourceSerializesNestedGoSyntaxAndRanges(t *testing.T) {
	host := "package p\nfunc greet() { _ = \"snow: \\u96ea\" }\n"
	parsed, _ := indexSource(t, host)
	var tree map[string]any
	require.NoError(t, json.Unmarshal(parsed.AST, &tree))
	decls := tree["Decls"].([]any)
	fn := decls[0].(map[string]any)
	require.Equal(t, "FuncDecl", fn["kind"])
	require.Equal(t, "greet", fn["Name"].(map[string]any)["Name"])
	entry, ok := parsed.Mapping["/Decls/0"]
	require.True(t, ok, "function node has no source range: %#v", entry)
	require.Len(t, entry.Range, 2)
	start, end := int(entry.Range[0]), int(entry.Range[1])
	require.Equal(t, "func greet() { _ = \"snow: \\u96ea\" }", host[start:end])
	_, ok = parsed.Mapping["/Decls/0/Body/List/0"]
	require.True(t, ok, "nested statement did not receive an independent mapping entry")
}

func TestIndexReturnsSourceAndNativeFragment(t *testing.T) {
	_, result := indexSource(t, "package example\nfunc greet() { _ = \"snow: \\u96ea\" }\n")
	require.Len(t, result.Fragments, 1, "incomplete=%+v", result.Incomplete)
	_, ok := result.Sources[0].Mapping["/Decls/0/Name"]
	require.True(t, ok, "AST serialization did not retain nested field mappings")
}

func TestSourceMappingPointersEscapeJSONPointerTokens(t *testing.T) {
	parsed, _ := indexSource(t, "package p\ntype T struct { Value string }\n")
	_, ok := parsed.Mapping["/Decls/0/Specs/0/Type/Fields/List/0/Names/0"]
	require.True(t, ok, "field declaration is missing a pointer-keyed mapping entry")
}

func indexSource(t *testing.T, host string) (sdk.Source, sdk.IndexResult) {
	t.Helper()
	root := t.TempDir()
	require.NoError(t, os.WriteFile(filepath.Join(root, "example.go"), []byte(host), 0600))
	result, err := provider.New("lang-go", "test").Index(sdk.IndexParams{
		Project:  sdk.ProjectRef{Root: root},
		Language: "go",
		Inputs: []sdk.IndexInput{{
			Kind:      "file",
			File:      &sdk.FileRef{Path: "example.go", Hash: "test"},
			SourceAST: true,
		}},
		Context: sdk.Context{Options: map[string]json.RawMessage{}},
	})
	require.NoError(t, err)
	require.Len(t, result.Sources, 1, "incomplete=%+v", result.Incomplete)
	return result.Sources[0], result
}
