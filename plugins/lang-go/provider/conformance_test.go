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

func TestInitializeResultMatchesTheSchema(t *testing.T) {
	p := provider.New("lang-go", "test")
	result, err := p.Initialize(sdk.InitializeParams{Root: "/", ProtocolVersion: sdk.ProtocolVersion})
	if err != nil {
		t.Fatal(err)
	}
	assertValid(t, validator(t, "InitializeResult"), result, "initialize result")
	if _, err := p.Initialize(sdk.InitializeParams{ProtocolVersion: "9.9"}); err == nil {
		t.Error("another protocol version must be refused")
	}
}

func TestIndexResultsMatchTheSchemaForEveryConformanceCase(t *testing.T) {
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
			t.Run(filepath.Base(dir)+"/"+variant, func(t *testing.T) {
				result, err := provider.New("lang-go", "test").Index(sdk.IndexParams{
					Project:  sdk.ProjectRef{Root: root},
					Language: "go",
					Files:    files,
					Context:  sdk.Context{Options: options},
				})
				if err != nil {
					t.Fatal(err)
				}
				assertValid(t, schema, result, "index result")
			})
		}
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

func TestSchemaRejectsWhatTheWireForbids(t *testing.T) {
	schema := validator(t, "IndexResult")
	for name, result := range map[string]sdk.IndexResult{
		"null lists": {},
		"unknown symbol kind": {
			Fragments: []sdk.Fragment{{Symbols: []sdk.Symbol{{ID: "m::x#function", Kind: "bogus", Visibility: "public"}}}},
			Notices:   []string{}, Incomplete: []sdk.Incomplete{},
		},
	} {
		raw, _ := json.Marshal(result)
		var instance any
		_ = json.Unmarshal(raw, &instance)
		if schema.Validate(instance) == nil {
			t.Errorf("%s: the schema accepted an invalid result", name)
		}
	}
}
