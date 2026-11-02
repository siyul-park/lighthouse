package provider

import (
	"encoding/json"
	"go/ast"
	"go/parser"
	"go/token"
	"maps"
	"os"
	"path"
	"path/filepath"
	"slices"
	"strings"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

// syntacticFact marks the fragment of a file the build context excludes: it
// holds the test entry points the file declares and nothing the go command's
// type check would add, so no rule judges the file, and tests in it count for
// the symbols they are named after but not as a second owner.
const syntacticFact = "syntactic"

// excludedTests reads the test files among the candidates that the build
// context excludes, once every other file has its fragment.
func (r *run) excludedTests(candidates []string) {
	for _, rel := range candidates {
		if _, failed := r.problems[rel]; !failed && !r.claimed[rel] && r.excludedFiles[rel] && strings.HasSuffix(rel, "_test.go") {
			r.syntacticTests(rel)
		}
	}
}

// syntacticTests gives an excluded test file the fragment its source says
// without a type check: a test symbol for every entry point and a test case
// for every `TestXxx`, whose targets are the symbols of the tested package the
// Go naming convention gives (`TestFoo` is `Foo`, `TestType_Method` is
// `Type.Method`); the host credits the owner by the same names. A file that
// cannot be parsed keeps its empty fragment.
func (r *run) syntacticTests(rel string) {
	abs := filepath.Join(r.root, filepath.FromSlash(rel))
	src, ok := r.overlays[abs]
	if !ok {
		src, _ = os.ReadFile(abs)
	}
	fset := token.NewFileSet()
	file, err := parser.ParseFile(fset, abs, src, parser.ParseComments|parser.SkipObjectResolution)
	if err != nil {
		return
	}
	dir := path.Dir(rel)
	module := sdk.Module{Path: dir, Name: file.Name.Name}
	if strings.HasSuffix(file.Name.Name, "_test") {
		module.Path, module.TestOf = dir+moduleSuffixTest, dir
	}
	frag := emptyFragment(sdk.FileInfo{Path: rel, Facts: map[string]string{syntacticFact: "true"}})
	maps.Copy(frag.File.Facts, stemFacts(rel))
	frag.Modules = []sdk.Module{module}
	for _, decl := range file.Decls {
		d, ok := decl.(*ast.FuncDecl)
		if ok && d.Recv == nil && isTestEntry(d.Name.Name, testEntryPrefixes...) {
			id, name := symbolID(kindTest, module.Path, d.Name.Name), d.Name.Name
			frag.Symbols = append(frag.Symbols, sdk.Symbol{ID: id, Kind: kindTest, Name: name, Doc: docText(d.Doc), File: rel, Span: span(fset, d.Pos(), d.End()), Visibility: visibilityOf(name, file.Name.Name == "main", hasComponent(dir, "internal"))})
			if isTestEntry(name, "Test") {
				frag.Tests = append(frag.Tests, sdk.TestCase{Symbol: id, Style: styleScenario, Targets: r.targetsNamed(dir, strings.TrimPrefix(name, "Test"))})
			}
		}
	}
	r.fragments[rel] = frag
}

// targetsNamed are the edge targets of the symbols of the package at dir that
// tests name `name`: a function, type or interface by its name, a method as
// `Type_Method`. Fragments the cache answered are JSON not yet decoded.
func (r *run) targetsNamed(dir, name string) []string {
	targets := []string{}
	fragments := slices.Collect(maps.Values(r.fragments))
	for _, raw := range r.encoded {
		var f sdk.Fragment
		if json.Unmarshal(raw, &f) == nil {
			fragments = append(fragments, &f)
		}
	}
	for _, f := range fragments {
		for _, s := range f.Symbols {
			target, kind, _ := strings.Cut(s.ID, "#")
			rest, ok := strings.CutPrefix(target, dir+"::")
			if ok && strings.ReplaceAll(rest, "::", "_") == name && slices.Contains([]string{kindFunction, kindMethod, kindType, kindInterface}, kind) {
				targets = append(targets, target)
			}
		}
	}
	slices.Sort(targets)
	return targets
}
