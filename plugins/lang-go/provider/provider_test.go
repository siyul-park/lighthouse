package provider_test

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"testing"

	"github.com/santhosh-tekuri/jsonschema/v6"
	"github.com/siyul-park/lighthouse/plugins/lang-go/provider"
	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
	"github.com/stretchr/testify/require"
)

// answer is what one request answered, its units together.
type answer struct {
	Units      []sdk.UnitResult `json:"units"`
	Fragments  []sdk.Fragment   `json:"fragments"`
	Sources    []sdk.Source     `json:"sources"`
	Notices    []string         `json:"notices"`
	Incomplete []sdk.Incomplete `json:"incomplete"`
}

// testModule is the go.mod of the projects the tests write.
const testModule = "module example.com/m\n\ngo 1.22\n"

const (
	schemaPath = "../../../protocol/schema/lighthouse-protocol-0.9.json"
	casesGlob  = "../../../crates/lighthouse-rpc/tests/conformance/go/*"
)

func together(result sdk.IndexResult) answer {
	got := answer{Units: result.Units, Notices: result.Notices, Incomplete: result.Incomplete}
	for _, unit := range result.Units {
		got.Fragments = append(got.Fragments, unit.Fragments...)
		got.Sources = append(got.Sources, unit.Sources...)
		got.Notices = append(got.Notices, unit.Notices...)
		got.Incomplete = append(got.Incomplete, unit.Incomplete...)
	}
	return got
}

// indexOf indexes under params and puts the units together.
func indexOf(g *provider.Provider, params sdk.IndexParams) (answer, error) {
	result, err := g.Index(params)
	return together(result), err
}

// oneUnit is one unit that holds the files.
func oneUnit(files []sdk.FileRef) []sdk.UnitRef {
	return []sdk.UnitRef{{ID: ".", Files: files}}
}

func filesOf(files []sdk.FileRef) []string {
	paths := make([]string, 0, len(files))
	for _, file := range files {
		paths = append(paths, file.Path)
	}
	return paths
}

func TestProviderManifest(t *testing.T) {
	t.Run("describes Go as the protocol schema defines it", func(t *testing.T) {
		g := provider.New("lang-go", "test")
		result := sdk.InitializeResult{
			ProtocolVersion: sdk.ProtocolVersion,
			ServerInfo:      sdk.Implementation{Name: g.Name(), Version: g.Version()},
			Capabilities:    sdk.ServerCapabilities{Languages: g.Manifest()},
		}

		assertValid(t, validator(t, "InitializeResult"), result, "initialize result")
	})
}

func TestProviderIndex(t *testing.T) {
	t.Run("matches the protocol schema for every conformance case", func(t *testing.T) {
		schema := validator(t, "IndexResult")
		cases, err := filepath.Glob(casesGlob)
		require.NoError(t, err)
		require.GreaterOrEqual(t, len(cases), 12, "conformance cases went missing")
		for _, dir := range cases {
			root, err := filepath.Abs(filepath.Join(dir, "project"))
			require.NoError(t, err)
			files := goFiles(t, root)
			for variant, options := range variants(t, dir) {
				result, err := provider.New("lang-go", "test").Index(sdk.IndexParams{
					Project:  sdk.ProjectRef{Root: root},
					Language: "go",
					Units:    oneUnit(files),
					Files:    filesOf(files),
					Context:  sdk.Context{Options: options},
				})
				require.NoError(t, err, "%s/%s", filepath.Base(dir), variant)
				assertValid(t, schema, result, filepath.Base(dir)+"/"+variant+" index result")
			}
		}
	})

	t.Run("indexes standalone files without a module", func(t *testing.T) {
		root := t.TempDir()
		path := filepath.Join(root, "p.go")
		require.NoError(t, os.WriteFile(path, []byte("package p\n\nfunc Run() {}\n"), 0o644))

		result, err := indexOf(provider.New("lang-go", "test"), sdk.IndexParams{
			Project:  sdk.ProjectRef{Root: root},
			Language: "go",
			Units:    oneUnit([]sdk.FileRef{{Path: "p.go"}}),
			Files:    filesOf([]sdk.FileRef{{Path: "p.go"}}),
		})

		require.NoError(t, err)
		require.Empty(t, result.Incomplete)
		require.Len(t, result.Fragments, 1)
		var names []string
		for _, symbol := range result.Fragments[0].Symbols {
			names = append(names, symbol.Name)
		}
		require.Contains(t, names, "Run")
	})

	t.Run("classifies entry points the way go test does", func(t *testing.T) {
		root := t.TempDir()
		write := func(name, body string) {
			require.NoError(t, os.WriteFile(filepath.Join(root, name), []byte(body), 0o644))
		}
		write("go.mod", "module example.com/p\n\ngo 1.22\n")
		write("p.go", "package p\n")
		write("p_test.go", `package p

import "testing"

func Test(t *testing.T)               {}
func TestAdd(t *testing.T)            {}
func Test_add(t *testing.T)           {}
func Testing(t *testing.T)            {}
func FuzzParse(f *testing.F)          {}
func ExampleRun()                     {}
func Examples()                       {}
func BenchmarkX(b *testing.B)         {}
func Benchmarking(b *testing.B)       {}
func helper(t *testing.T)             {}
`)
		g := provider.New("lang-go", "test")

		result, err := indexOf(g, sdk.IndexParams{
			Project:  sdk.ProjectRef{Root: root},
			Language: "go",
			Units:    oneUnit([]sdk.FileRef{{Path: "p_test.go"}, {Path: "p.go"}}),
			Files:    filesOf([]sdk.FileRef{{Path: "p_test.go"}, {Path: "p.go"}}),
		})

		require.NoError(t, err)
		kinds := map[string]string{}
		cases := []string{}
		for _, f := range result.Fragments {
			for _, s := range f.Symbols {
				kinds[s.Name] = s.Kind
			}
			for _, c := range f.Tests {
				cases = append(cases, c.Symbol)
			}
		}
		want := map[string]string{
			"Test": "test", "TestAdd": "test", "Test_add": "test", "Testing": "function",
			"FuzzParse": "test", "ExampleRun": "test", "Examples": "function",
			"BenchmarkX": "test", "Benchmarking": "function", "helper": "function",
		}
		for name, kind := range want {
			require.Equal(t, kind, kinds[name], name)
		}
		require.Len(t, cases, 3, "only Test, TestAdd and Test_add are test cases")
	})

	t.Run("a package variable's initializer uses what it names", func(t *testing.T) {
		root := t.TempDir()
		require.NoError(t, os.WriteFile(filepath.Join(root, "go.mod"), []byte("module example.com/p\n\ngo 1.22\n"), 0o644))
		require.NoError(t, os.WriteFile(filepath.Join(root, "p.go"), []byte(
			"package p\n\nfunc run() {}\n\nfunc build() int { return 1 }\n\nvar handlers = map[string]func(){\"a\": run}\n\nvar size = build()\n"), 0o644))

		result, err := indexOf(provider.New("lang-go", "test"), sdk.IndexParams{
			Project:  sdk.ProjectRef{Root: root},
			Language: "go",
			Units:    oneUnit(goFiles(t, root)),
			Files:    filesOf(goFiles(t, root)),
		})

		require.NoError(t, err)
		require.Empty(t, result.Incomplete)
		uses := map[string]string{}
		for _, f := range result.Fragments {
			for _, e := range f.Edges {
				if e.From.Symbol != "" {
					uses[e.From.Symbol+" "+e.Kind] = e.To
				}
			}
		}
		require.Equal(t, ".::run", uses[".::handlers#var references"])
		require.Equal(t, ".::build", uses[".::size#var calls"])
	})

	t.Run("reports an unknown option key as incomplete", func(t *testing.T) {
		g := provider.New("lang-go", "test")
		options := map[string]json.RawMessage{"go": json.RawMessage(`{"tagz":[]}`)}

		result, err := indexOf(g, sdk.IndexParams{Project: sdk.ProjectRef{Root: t.TempDir()}, Context: sdk.Context{Options: options}})

		require.NoError(t, err)
		require.Len(t, result.Incomplete, 1)
		require.Contains(t, result.Incomplete[0].Reason, "tagz")
	})

	t.Run("ignores the options of another language", func(t *testing.T) {
		g := provider.New("lang-go", "test")
		options := map[string]json.RawMessage{"python": json.RawMessage(`{"anything":1}`)}

		result, err := indexOf(g, sdk.IndexParams{Project: sdk.ProjectRef{Root: t.TempDir()}, Context: sdk.Context{Options: options}})

		require.NoError(t, err)
		require.Empty(t, result.Incomplete)
	})

	t.Run("a toolchain missing from PATH leaves the files incomplete, never clean", func(t *testing.T) {
		root := t.TempDir()
		require.NoError(t, os.WriteFile(filepath.Join(root, "go.mod"), []byte("module example.com/p\n\ngo 1.22\n"), 0o644))
		require.NoError(t, os.WriteFile(filepath.Join(root, "p.go"), []byte("package p\n"), 0o644))
		t.Setenv("PATH", t.TempDir())
		t.Setenv("GOROOT", "")

		result, err := indexOf(provider.New("lang-go", "test"), sdk.IndexParams{
			Project:  sdk.ProjectRef{Root: root},
			Language: "go",
			Units:    oneUnit(goFiles(t, root)),
			Files:    filesOf(goFiles(t, root)),
		})

		require.NoError(t, err)
		require.NotEmpty(t, result.Incomplete, "no go command must not look like a clean run")
		require.Contains(t, result.Incomplete[0].Reason, "go")
		for _, f := range result.Fragments {
			require.Empty(t, f.Symbols, "nothing is claimed of a file that was not analyzed")
		}
	})

	t.Run("reports a missing go command as incomplete", func(t *testing.T) {
		g := provider.New("lang-go", "test")
		options := map[string]json.RawMessage{"go": json.RawMessage(`{"go":"/nonexistent/go"}`)}

		result, err := indexOf(g, sdk.IndexParams{Project: sdk.ProjectRef{Root: t.TempDir()}, Context: sdk.Context{Options: options}})

		require.NoError(t, err)
		require.Len(t, result.Incomplete, 1)
		require.Contains(t, result.Incomplete[0].Reason, "go command not found")
	})
}

func TestSchemaRejectsWhatTheWireForbids(t *testing.T) {
	schema := validator(t, "IndexResult")
	for _, tc := range []struct {
		name   string
		result sdk.IndexResult
	}{
		{name: "null lists", result: sdk.IndexResult{}},
		{name: "unknown symbol kind", result: sdk.IndexResult{
			Units: []sdk.UnitResult{{
				ID: "u", Body: "b", Api: "a", Notices: []string{}, Incomplete: []sdk.Incomplete{},
				Fragments: []sdk.Fragment{{Symbols: []sdk.Symbol{{ID: "m::x#function", Kind: "bogus", Visibility: "public"}}}},
			}},
			Notices: []string{}, Incomplete: []sdk.Incomplete{},
		}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			raw, err := json.Marshal(tc.result)
			require.NoError(t, err)
			var instance any
			require.NoError(t, json.Unmarshal(raw, &instance))

			require.Error(t, schema.Validate(instance), "the schema accepted an invalid result")
		})
	}
}

// validator checks values against one definition of the checked-in schema,
// the contract other SDKs generate their types from.
func validator(t *testing.T, definition string) *jsonschema.Schema {
	t.Helper()
	raw, err := os.ReadFile(schemaPath)
	require.NoError(t, err)
	var schema map[string]any
	require.NoError(t, json.Unmarshal(raw, &schema))
	doc := map[string]any{
		"$schema": schema["$schema"],
		"$ref":    "#/$defs/" + definition,
		"$defs":   schema["$defs"],
	}
	c := jsonschema.NewCompiler()
	require.NoError(t, c.AddResource("schema.json", doc))
	compiled, err := c.Compile("schema.json")
	require.NoError(t, err)
	return compiled
}

func assertValid(t *testing.T, s *jsonschema.Schema, value any, what string) {
	t.Helper()
	raw, err := json.Marshal(value)
	require.NoError(t, err)
	var instance any
	require.NoError(t, json.Unmarshal(raw, &instance))
	require.NoError(t, s.Validate(instance), "%s violates the schema", what)
}

// variants maps each `options[.name].json` of a case to its options; a case
// without options.json runs once with none.
func variants(t *testing.T, dir string) map[string]map[string]json.RawMessage {
	t.Helper()
	found := map[string]map[string]json.RawMessage{}
	paths, _ := filepath.Glob(filepath.Join(dir, "options*.json"))
	for _, path := range paths {
		name := strings.TrimSuffix(strings.TrimPrefix(filepath.Base(path), "options"), ".json")
		raw, err := os.ReadFile(path)
		require.NoError(t, err)
		var options map[string]json.RawMessage
		require.NoError(t, json.Unmarshal(raw, &options))
		found[strings.TrimPrefix(name, ".")] = options
	}
	if _, ok := found[""]; !ok {
		found[""] = map[string]json.RawMessage{}
	}
	return found
}

// indexWith indexes the project as one unit and gives what it answered as
// JSON, with the stats of the request.
func indexWith(t *testing.T, g *provider.Provider, root string, options map[string]json.RawMessage) (string, provider.RunStats) {
	t.Helper()
	if options == nil {
		options = map[string]json.RawMessage{}
	}
	files := hashedGoFiles(t, root)
	result, err := indexOf(g, sdk.IndexParams{
		Project:  sdk.ProjectRef{Root: root},
		Language: "go",
		Units:    oneUnit(files),
		Files:    filesOf(files),
		Context:  sdk.Context{Options: options},
	})
	require.NoError(t, err)
	result.Units = nil
	raw, err := json.Marshal(result)
	require.NoError(t, err)
	return string(raw), g.Stats()
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

func goFiles(t *testing.T, root string) []sdk.FileRef {
	t.Helper()
	var files []sdk.FileRef
	err := filepath.WalkDir(root, func(path string, d fs.DirEntry, err error) error {
		if err == nil && !d.IsDir() && strings.HasSuffix(path, ".go") {
			rel, _ := filepath.Rel(root, path)
			files = append(files, sdk.FileRef{Path: filepath.ToSlash(rel), Hash: "unused"})
		}
		return err
	})
	require.NoError(t, err)
	sort.Slice(files, func(i, j int) bool { return files[i].Path < files[j].Path })
	return files
}
