package provider

import (
	"encoding/json"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"path"
	"path/filepath"
	"sort"
	"strings"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
	"golang.org/x/tools/imports"
)

// snippet is one fence as a unit the go command loads.
type snippet struct {
	host   *sdk.EmbeddedFile
	source *sdk.EmbeddedSource
	// rel is the project-relative path of the unit's file.
	rel string
	// module is the module path the unit declares its symbols in.
	module string
	// text is what the fence is compiled as.
	text regionText
	// note says why the fence is not analysed, when it is not.
	note string
}

// snippetsDir holds the units of the fences beside their Markdown file. No
// disk has them: each is an overlay, one package of its own.
const snippetsDir = ".lighthouse-snippets"

// packageKey is the context key that puts a fence inside the package of a
// directory instead of in a package of its own.
const packageKey = "package"

// indexRegions analyses the Go fences of host files, each a unit of its own
// that uses the module around the Markdown file from outside, as an importer
// does. The units of a module are loaded together, so N fences cost one load.
// A fence that is only a fragment is a notice and not a gap: the names it
// defines in prose do not resolve, and what does is reported.
func (p *Provider) indexRegions(params sdk.IndexParams) sdk.IndexResult {
	opts, err := newOptions(params.Context.Options)
	if err != nil {
		result := emptyResult()
		result.Incomplete = append(result.Incomplete, sdk.Incomplete{Reason: err.Error()})
		return result
	}
	r := newRun(params, opts)
	var snippets []*snippet
	for _, input := range params.Inputs {
		if input.Kind == "regions" && input.Regions != nil {
			snippets = append(snippets, r.snippets(input.Regions)...)
		}
	}
	for _, s := range snippets {
		if s.note != "" {
			continue
		}
		r.requested[s.rel] = sdk.FileRef{Path: s.rel}
		r.overlays[filepath.Join(r.root, filepath.FromSlash(s.rel))] = s.text.text
		r.regionModules[s.rel] = s.module
	}
	r.index()
	p.last = RunStats{Loads: r.loads}
	return r.regionResult(snippets)
}

// snippets are the units of the fences of one host file, in the order of the
// fences.
func (r *run) snippets(host *sdk.EmbeddedFile) []*snippet {
	sources := make([]*sdk.EmbeddedSource, len(host.Sources))
	for i := range host.Sources {
		sources[i] = &host.Sources[i]
	}
	sort.SliceStable(sources, func(i, j int) bool { return sources[i].SourceRange[0] < sources[j].SourceRange[0] })
	var out []*snippet
	cache := map[string]string{}
	for i, source := range sources {
		s := &snippet{host: host, source: source}
		dir := path.Dir(host.Path)
		name := fmt.Sprintf("%s.%d", path.Base(host.Path), i+1)
		s.module = path.Join(dir, snippetsDir, name)
		s.rel = path.Join(s.module, "snippet.go")
		pkg, wrapper := "main", "main"
		if into := contextString(source, packageKey); into != "" {
			pkg, wrapper = r.packageName(into), fmt.Sprintf("snippet%d", i+1)
			s.rel = path.Join(into, "zz_lighthouse_"+sanitize(name)+".go")
			s.module = path.Join(into, snippetsDir, name)
			if pkg == "" {
				s.note = fmt.Sprintf("names `%s` in `language.context.%s`, which holds no Go package", into, packageKey)
				out = append(out, s)
				continue
			}
		}
		moduleDir, ok := nearestModule(filepath.Join(r.root, filepath.FromSlash(dir)), cache)
		if !ok {
			moduleDir = r.root
		}
		r.compile(s, pkg, wrapper, moduleDir)
		out = append(out, s)
	}
	return out
}

// compile gives the text a fence is compiled as: a file as it is, declarations
// under a package clause, statements in a function, and the imports that name
// what the fence uses without importing.
func (r *run) compile(s *snippet, pkg, wrapper, moduleDir string) {
	body := newRegionText(s.source)
	src := s.source.Text
	if parses(src, parser.PackageClauseOnly) {
		s.text = body
		return
	}
	head := "package " + pkg + "\n\n"
	var form func(imports string) regionText
	switch {
	case parses("package p\n"+src, 0):
		form = func(imports string) regionText { return body.around(head+imports, "") }
	case parses("package p\n\nfunc _() {\n"+src+"\n}\n", 0):
		form = func(imports string) regionText {
			return body.around(head+imports+"func "+wrapper+"() {\n", "\n}\n")
		}
	default:
		s.note = "is not valid Go"
		return
	}
	s.text = form("")
	if missing := r.missingImports(moduleDir, s.text.text); missing != "" {
		s.text = form(missing)
	}
}

// packageName is the name of the Go package in the directory, "" when it holds
// none.
func (r *run) packageName(dir string) string {
	entries, err := os.ReadDir(filepath.Join(r.root, filepath.FromSlash(dir)))
	if err != nil {
		return ""
	}
	for _, entry := range entries {
		name := entry.Name()
		if !strings.HasSuffix(name, ".go") || strings.HasSuffix(name, "_test.go") {
			continue
		}
		file, err := parser.ParseFile(token.NewFileSet(), filepath.Join(r.root, filepath.FromSlash(dir), name), nil, parser.PackageClauseOnly)
		if err == nil {
			return file.Name.Name
		}
	}
	return ""
}

// missingImports is the import declaration goimports adds to the text, for the
// packages it names without importing; empty when it uses none. What goimports
// settles depends on the names and the module only, so each set of names is
// asked once.
func (r *run) missingImports(dir string, text []byte) string {
	file, err := parser.ParseFile(token.NewFileSet(), "", text, 0)
	if err != nil {
		return ""
	}
	names := qualifiers(file)
	if len(names) == 0 {
		return ""
	}
	key := dir + "\x00" + strings.Join(names, ",")
	if known, ok := r.imported[key]; ok {
		return known
	}
	declaration := goimports(dir, text)
	r.imported[key] = declaration
	return declaration
}

// regionResult reports the fragments of the units in the host's coordinates,
// one fragment per host file.
func (r *run) regionResult(snippets []*snippet) sdk.IndexResult {
	merged := map[string]*sdk.Fragment{}
	var untyped []string
	result := r.result
	// A host file with no fence worth reading has a fragment all the same.
	for _, s := range snippets {
		mergeFragment(merged, emptyFragment(sdk.FileInfo{Path: s.host.Path}), s.host.Path)
	}
	for _, s := range snippets {
		where := fmt.Sprintf("%s:%d", s.host.Path, positionAt(lineStarts([]byte(s.host.Text)), int(s.source.SourceRange[0])).Line)
		if s.note != "" {
			if strings.HasPrefix(s.note, "names ") {
				result.Incomplete = append(result.Incomplete, sdk.Incomplete{Path: s.host.Path, Reason: s.note})
				continue
			}
			result.Notices = append(result.Notices, fmt.Sprintf("%s: the Go fence %s and is not analysed", where, s.note))
			continue
		}
		if reason := r.problems[s.rel]; reason != "" {
			untyped = append(untyped, where+": "+reason)
		}
		fragment := r.fragments[s.rel]
		if fragment == nil || len(fragment.Modules) == 0 {
			continue
		}
		m := remap{text: s.text, lines: lineStarts(s.text.text), host: lineStarts([]byte(s.host.Text)), module: s.module}
		mergeFragment(merged, m.fragment(fragment, s.host.Path), s.host.Path)
	}
	if len(untyped) > 0 {
		result.Notices = append(result.Notices, fmt.Sprintf(
			"%d Go fence(s) do not type-check (names the prose defines, or code that is not meant to compile); what resolves is reported, e.g. %s",
			len(untyped), untyped[0]))
	}
	paths := make([]string, 0, len(merged))
	for p := range merged {
		paths = append(paths, p)
	}
	sort.Strings(paths)
	result.Fragments = result.Fragments[:0]
	for _, p := range paths {
		result.Fragments = append(result.Fragments, *merged[p])
	}
	return result
}

// mergeFragment adds the fragment of a fence to the one of its host file.
func mergeFragment(merged map[string]*sdk.Fragment, f *sdk.Fragment, host string) {
	into := merged[host]
	if into == nil {
		into = emptyFragment(sdk.FileInfo{Path: host})
		merged[host] = into
	}
	into.Modules = append(into.Modules, f.Modules...)
	into.Symbols = append(into.Symbols, f.Symbols...)
	into.Edges = append(into.Edges, f.Edges...)
	into.Functions = append(into.Functions, f.Functions...)
	into.Tests = append(into.Tests, f.Tests...)
}

// goimports asks the vendor tool which imports the text lacks, as its module
// has them.
func goimports(dir string, text []byte) string {
	cwd, err := os.Getwd()
	if err != nil || os.Chdir(dir) != nil {
		return ""
	}
	defer func() { _ = os.Chdir(cwd) }()
	out, err := imports.Process(filepath.Join(dir, "lighthouse_snippet.go"), text, &imports.Options{Comments: true, TabIndent: true, TabWidth: 8})
	if err != nil {
		return ""
	}
	added, err := parser.ParseFile(token.NewFileSet(), "", out, parser.ImportsOnly)
	if err != nil || len(added.Imports) == 0 {
		return ""
	}
	var b strings.Builder
	b.WriteString("import (\n")
	for _, spec := range added.Imports {
		b.WriteString("\t")
		if spec.Name != nil {
			b.WriteString(spec.Name.Name + " ")
		}
		b.WriteString(spec.Path.Value + "\n")
	}
	b.WriteString(")\n\n")
	return b.String()
}

// qualifiers are the names of the packages the file uses without importing:
// the identifiers no declaration binds that something is selected from.
func qualifiers(file *ast.File) []string {
	unresolved := map[*ast.Ident]bool{}
	for _, id := range file.Unresolved {
		unresolved[id] = true
	}
	found := map[string]bool{}
	ast.Inspect(file, func(n ast.Node) bool {
		if sel, ok := n.(*ast.SelectorExpr); ok {
			if id, ok := sel.X.(*ast.Ident); ok && unresolved[id] {
				found[id.Name] = true
			}
		}
		return true
	})
	names := make([]string, 0, len(found))
	for name := range found {
		names = append(names, name)
	}
	sort.Strings(names)
	return names
}

func parses(src string, mode parser.Mode) bool {
	_, err := parser.ParseFile(token.NewFileSet(), "", src, mode)
	return err == nil
}

// contextString is the string a source rule gave under key.
func contextString(source *sdk.EmbeddedSource, key string) string {
	var context map[string]json.RawMessage
	if json.Unmarshal(source.Context, &context) != nil {
		return ""
	}
	var value string
	if json.Unmarshal(context[key], &value) != nil {
		return ""
	}
	return value
}

// sanitize keeps the letters and digits of a name, so that it is one element of
// a file name.
func sanitize(name string) string {
	return strings.Map(func(c rune) rune {
		if c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' {
			return c
		}
		return '_'
	}, name)
}
