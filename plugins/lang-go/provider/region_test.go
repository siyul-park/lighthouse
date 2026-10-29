package provider_test

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/siyul-park/lighthouse/plugins/lang-go/provider"
	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
	"github.com/stretchr/testify/require"
)

type fence struct {
	body    string
	context string
}

const (
	regionModule = "module example.com/m\n\ngo 1.22\n"
	regionLib    = "package m\n\nfunc Hello() string { return \"x\" }\n\nfunc helper() {}\n"
)

func hasNotice(result sdk.IndexResult, part string) bool {
	for _, notice := range result.Notices {
		if strings.Contains(notice, part) {
			return true
		}
	}
	return false
}

func TestProviderIndexRegions(t *testing.T) {
	t.Run("a fence that calls the module's exported API resolves it, at its place in the host", func(t *testing.T) {
		root := regionProject(t)

		result, g := indexFences(t, root, []fence{{body: "fmt.Println(m.Hello())\n"}})

		require.Empty(t, result.Incomplete)
		require.Equal(t, 1, g.Stats().Loads)
		call := edgeTo(t, result, "Hello")
		require.Equal(t, "calls", call.Kind)
		require.NotNil(t, call.Site)
		require.Equal(t, sdk.Position{Line: 2, Col: 15}, call.Site.Start)
	})

	t.Run("the wrapper of statements is no symbol and no place", func(t *testing.T) {
		root := regionProject(t)

		result, _ := indexFences(t, root, []fence{{body: "fmt.Println(m.Hello())\n"}})

		fragment := onlyFragment(t, result)
		for _, symbol := range fragment.Symbols {
			require.NotEqual(t, "main", symbol.Name)
		}
		for _, edge := range fragment.Edges {
			if edge.Site != nil {
				require.GreaterOrEqual(t, edge.Site.Start.Line, 2)
			}
		}
	})

	t.Run("declarations are symbols at their place", func(t *testing.T) {
		root := regionProject(t)

		result, _ := indexFences(t, root, []fence{{body: "type Sample struct{}\n"}})

		fragment := onlyFragment(t, result)
		require.Len(t, fragment.Symbols, 1)
		require.Equal(t, "Sample", fragment.Symbols[0].Name)
		require.Equal(t, 2, fragment.Symbols[0].Span.Start.Line)
		require.Equal(t, "README.md", fragment.Symbols[0].File)
	})

	t.Run("identical fences are distinct units", func(t *testing.T) {
		root := regionProject(t)

		result, _ := indexFences(t, root, []fence{{body: "type Same struct{}\n"}, {body: "type Same struct{}\n"}})

		fragment := onlyFragment(t, result)
		var ids []string
		for _, symbol := range fragment.Symbols {
			ids = append(ids, symbol.ID)
		}
		require.Len(t, ids, 2)
		require.NotEqual(t, ids[0], ids[1])
	})

	t.Run("a fragment that names what the prose defines is a notice and no gap", func(t *testing.T) {
		root := regionProject(t)

		result, _ := indexFences(t, root, []fence{{body: "value := compute(input)\n_ = value\n"}})

		require.Empty(t, result.Incomplete)
		require.True(t, hasNotice(result, "do not type-check"), "%v", result.Notices)
	})

	t.Run("a fence that is not Go is a notice", func(t *testing.T) {
		root := regionProject(t)

		result, _ := indexFences(t, root, []fence{{body: "this is { not go\n"}})

		require.Empty(t, result.Incomplete)
		require.True(t, hasNotice(result, "README.md:2: the Go fence is not valid Go"), "%v", result.Notices)
	})

	t.Run("the package context puts the fence in the package, where unexported names resolve", func(t *testing.T) {
		root := regionProject(t)

		result, _ := indexFences(t, root, []fence{{body: "helper()\n", context: `{"package":"."}`}})

		require.Empty(t, result.Incomplete)
		call := edgeTo(t, result, "helper")
		require.Equal(t, sdk.Position{Line: 2, Col: 1}, call.Site.Start)
	})

	t.Run("a directory without a Go package is a gap", func(t *testing.T) {
		root := regionProject(t)

		result, _ := indexFences(t, root, []fence{{body: "helper()\n", context: `{"package":"nowhere"}`}})

		require.Len(t, result.Incomplete, 1)
		require.Contains(t, result.Incomplete[0].Reason, "nowhere")
	})

	t.Run("places are bytes of the host whatever the line endings and letters", func(t *testing.T) {
		root := regionProject(t)

		result, _ := indexFences(t, root, []fence{{body: "// 안녕\r\nfmt.Println(m.Hello())\r\n"}})

		call := edgeTo(t, result, "Hello")
		require.Equal(t, sdk.Position{Line: 3, Col: 15}, call.Site.Start)
	})

	t.Run("N fences cost one load", func(t *testing.T) {
		root := regionProject(t)
		many := make([]fence, 24)
		for i := range many {
			many[i] = fence{body: fmt.Sprintf("fmt.Println(m.Hello(), %d)\n", i)}
		}

		started := time.Now()
		_, one := indexFences(t, root, many[:1])
		single := time.Since(started)
		started = time.Now()
		result, all := indexFences(t, root, many)
		batch := time.Since(started)

		require.Equal(t, 1, one.Stats().Loads)
		require.Equal(t, 1, all.Stats().Loads)
		require.Len(t, onlyFragment(t, result).Edges, 24*3, "two imports and one call per fence")
		t.Logf("1 fence %v, %d fences %v", single, len(many), batch)
		require.Less(t, batch, 12*single)
	})
}

func regionProject(t *testing.T) string {
	t.Helper()
	root := t.TempDir()
	require.NoError(t, os.WriteFile(filepath.Join(root, "go.mod"), []byte(regionModule), 0o644))
	require.NoError(t, os.WriteFile(filepath.Join(root, "m.go"), []byte(regionLib), 0o644))
	return root
}

// indexFences indexes the fences of README.md, written one after the other.
func indexFences(t *testing.T, root string, fences []fence) (sdk.IndexResult, *provider.Provider) {
	t.Helper()
	var host strings.Builder
	var sources []sdk.EmbeddedSource
	for i, f := range fences {
		host.WriteString("```go\n")
		from := host.Len()
		host.WriteString(f.body)
		to := host.Len()
		source := sdk.EmbeddedSource{
			ID:            fmt.Sprintf("fence-%d", i),
			RuleID:        "markdown-go",
			NodePointer:   fmt.Sprintf("/children/%d", i),
			SourcePointer: fmt.Sprintf("/children/%d/text", i),
			Provider:      "go",
			Text:          f.body,
			SourceRange:   [2]uint32{uint32(from), uint32(to)},
			TextMap: []sdk.TextMapSegment{{
				TextRange:   [2]uint32{0, uint32(len(f.body))},
				SourceRange: [2]uint32{uint32(from), uint32(to)},
			}},
		}
		if f.context != "" {
			source.Context = json.RawMessage(f.context)
		}
		sources = append(sources, source)
		host.WriteString("```\n\n")
	}
	g := provider.New("lang-go", "test")
	result, err := g.Index(sdk.IndexParams{
		Project:  sdk.ProjectRef{Root: root},
		Language: "go",
		Inputs: []sdk.IndexInput{{Kind: "regions", Regions: &sdk.EmbeddedFile{
			Path: "README.md", Text: host.String(), Sources: sources,
		}}},
	})
	require.NoError(t, err)
	return result, g
}

func edgeTo(t *testing.T, result sdk.IndexResult, name string) sdk.Edge {
	t.Helper()
	fragment := onlyFragment(t, result)
	for _, edge := range fragment.Edges {
		if strings.Contains(edge.To, "::"+name) {
			return edge
		}
	}
	require.FailNow(t, "no edge to "+name, "%+v", fragment.Edges)
	return sdk.Edge{}
}

func onlyFragment(t *testing.T, result sdk.IndexResult) sdk.Fragment {
	t.Helper()
	require.Len(t, result.Fragments, 1, "%v %v", result.Notices, result.Incomplete)
	return result.Fragments[0]
}
