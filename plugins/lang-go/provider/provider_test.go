package provider_test

import (
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
)

const (
	schemaPath = "../../../protocol/schema/lighthouse-protocol-0.1.json"
	casesGlob  = "../../conformance/go/*"
)

func TestGoInitialize(t *testing.T) {
	t.Run("describes Go as the protocol schema defines it", func(t *testing.T) {
		g := provider.New("lang-go", "test")

		result, err := g.Initialize(sdk.InitializeParams{Root: "/", ProtocolVersion: sdk.ProtocolVersion})

		if err != nil {
			t.Fatal(err)
		}
		assertValid(t, validator(t, "InitializeResult"), result, "initialize result")
	})

	t.Run("refuses another protocol version", func(t *testing.T) {
		g := provider.New("lang-go", "test")

		_, err := g.Initialize(sdk.InitializeParams{ProtocolVersion: "9.9"})

		if err == nil {
			t.Error("another protocol version must be refused")
		}
	})
}

func TestGoIndex(t *testing.T) {
	t.Run("matches the protocol schema for every conformance case", func(t *testing.T) {
		schema := validator(t, "IndexResult")
		cases, err := filepath.Glob(casesGlob)
		if err != nil || len(cases) < 12 {
			t.Fatalf("conformance cases went missing: %v (%d found)", err, len(cases))
		}
		for _, dir := range cases {
			root, err := filepath.Abs(filepath.Join(dir, "project"))
			if err != nil {
				t.Fatal(err)
			}
			files := goFiles(t, root)
			for variant, options := range variants(t, dir) {
				result, err := provider.New("lang-go", "test").Index(sdk.IndexParams{
					Project:  sdk.ProjectRef{Root: root},
					Language: "go",
					Files:    files,
					Context:  sdk.Context{Options: options},
				})
				if err != nil {
					t.Fatalf("%s/%s: %v", filepath.Base(dir), variant, err)
				}
				assertValid(t, schema, result, filepath.Base(dir)+"/"+variant+" index result")
			}
		}
	})

	t.Run("reports an unknown option key as incomplete", func(t *testing.T) {
		g := provider.New("lang-go", "test")
		options := map[string]json.RawMessage{"go": json.RawMessage(`{"tagz":[]}`)}

		result, err := g.Index(sdk.IndexParams{Project: sdk.ProjectRef{Root: t.TempDir()}, Context: sdk.Context{Options: options}})

		if err != nil {
			t.Fatal(err)
		}
		if len(result.Incomplete) != 1 || !strings.Contains(result.Incomplete[0].Reason, "tagz") {
			t.Errorf("incomplete = %+v, want one naming the unknown key", result.Incomplete)
		}
	})

	t.Run("ignores the options of another language", func(t *testing.T) {
		g := provider.New("lang-go", "test")
		options := map[string]json.RawMessage{"python": json.RawMessage(`{"anything":1}`)}

		result, err := g.Index(sdk.IndexParams{Project: sdk.ProjectRef{Root: t.TempDir()}, Context: sdk.Context{Options: options}})

		if err != nil {
			t.Fatal(err)
		}
		if len(result.Incomplete) != 0 {
			t.Errorf("incomplete = %+v, want none", result.Incomplete)
		}
	})

	t.Run("reports a missing go command as incomplete", func(t *testing.T) {
		g := provider.New("lang-go", "test")
		options := map[string]json.RawMessage{"go": json.RawMessage(`{"go":"/nonexistent/go"}`)}

		result, err := g.Index(sdk.IndexParams{Project: sdk.ProjectRef{Root: t.TempDir()}, Context: sdk.Context{Options: options}})

		if err != nil {
			t.Fatal(err)
		}
		if len(result.Incomplete) != 1 || !strings.Contains(result.Incomplete[0].Reason, "go command not found") {
			t.Errorf("incomplete = %+v, want one naming the missing command", result.Incomplete)
		}
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
			Fragments: []sdk.Fragment{{Symbols: []sdk.Symbol{{ID: "m::x#function", Kind: "bogus", Visibility: "public"}}}},
			Notices:   []string{}, Incomplete: []sdk.Incomplete{},
		}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			raw, _ := json.Marshal(tc.result)
			var instance any
			_ = json.Unmarshal(raw, &instance)

			if schema.Validate(instance) == nil {
				t.Error("the schema accepted an invalid result")
			}
		})
	}
}

// validator checks values against one definition of the checked-in schema,
// the contract other SDKs generate their types from.
func validator(t *testing.T, definition string) *jsonschema.Schema {
	t.Helper()
	raw, err := os.ReadFile(schemaPath)
	if err != nil {
		t.Fatal(err)
	}
	var schema map[string]any
	if err := json.Unmarshal(raw, &schema); err != nil {
		t.Fatal(err)
	}
	doc := map[string]any{
		"$schema": schema["$schema"],
		"$ref":    "#/$defs/" + definition,
		"$defs":   schema["$defs"],
	}
	c := jsonschema.NewCompiler()
	if err := c.AddResource("schema.json", doc); err != nil {
		t.Fatal(err)
	}
	compiled, err := c.Compile("schema.json")
	if err != nil {
		t.Fatal(err)
	}
	return compiled
}

func assertValid(t *testing.T, s *jsonschema.Schema, value any, what string) {
	t.Helper()
	raw, err := json.Marshal(value)
	if err != nil {
		t.Fatal(err)
	}
	var instance any
	if err := json.Unmarshal(raw, &instance); err != nil {
		t.Fatal(err)
	}
	if err := s.Validate(instance); err != nil {
		t.Errorf("%s violates the schema: %v", what, err)
	}
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
	if err != nil {
		t.Fatal(err)
	}
	sort.Slice(files, func(i, j int) bool { return files[i].Path < files[j].Path })
	return files
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
		if err != nil {
			t.Fatal(err)
		}
		var options map[string]json.RawMessage
		if err := json.Unmarshal(raw, &options); err != nil {
			t.Fatal(err)
		}
		found[strings.TrimPrefix(name, ".")] = options
	}
	if _, ok := found[""]; !ok {
		found[""] = map[string]json.RawMessage{}
	}
	return found
}
