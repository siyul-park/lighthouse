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
	"strconv"
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
	// form is the text of the fence under the imports it lacks, uses are
	// what it selects from packages it does not import (`name.Selected`),
	// and dir is the module they are looked up in.
	form func(imports string) regionText
	uses []string
	dir  string
	// shared says that the fence stands in a package of the project.
	shared bool
}

// importSpec is one import of a declaration.
type importSpec struct {
	name string
	path string
}

// snippetsDir holds the units of the fences beside their Markdown file. No
// disk has them: each is an overlay, one package of its own.
const snippetsDir = ".lighthouse-snippets"

// packageKey is the context key that puts a fence inside the package of a
// directory instead of in a package of its own.
const packageKey = "package"

// fences are the units of the Go fences of the host files of the request: each
// a unit of its own that uses the module around the Markdown file from outside,
// as an importer does, or a file of the package a source rule names. Their
// imports are completed for all of them at once.
func (r *run) fences(params sdk.IndexParams) []*snippet {
	var snippets []*snippet
	for _, input := range params.Inputs {
		if input.Kind == "regions" && input.Regions != nil {
			snippets = append(snippets, r.snippets(input.Regions)...)
		}
	}
	r.complete(snippets)
	return snippets
}

// admit has the run analyse the fences with its files, in the loads it makes
// for them: a fence is a file that stands in a directory beside its host.
func (r *run) admit(snippets []*snippet) {
	for _, s := range snippets {
		if s.note != "" {
			continue
		}
		r.requested[s.rel] = sdk.FileRef{Path: s.rel}
		r.overlays[filepath.Join(r.root, filepath.FromSlash(s.rel))] = s.text.text
		r.fenceFiles[s.rel] = true
		if s.module != "" {
			r.regionModules[s.rel] = s.module
		}
	}
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
			s.module, s.shared = "", true
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
	switch {
	case parses("package p\n"+src, 0):
		s.form = func(imports string) regionText { return body.around(head+imports, "") }
	case parses("package p\n\nfunc _() {\n"+src+"\n}\n", 0):
		s.form = func(imports string) regionText {
			return body.around(head+imports+"func "+wrapper+"() {\n", "\n}\n")
		}
	default:
		s.note = "is not valid Go"
		return
	}
	s.text = s.form("")
	s.dir = moduleDir
	if file, err := parser.ParseFile(token.NewFileSet(), "", s.text.text, 0); err == nil {
		s.uses = qualifiers(file)
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

// complete gives each fence the imports it lacks. What goimports settles
// depends on the names and the module only, and each call scans the module, so
// the names of all the fences of a module are asked at once.
func (r *run) complete(snippets []*snippet) {
	groups := map[string][]*snippet{}
	for _, s := range snippets {
		if s.form != nil && len(s.uses) > 0 {
			groups[s.dir] = append(groups[s.dir], s)
		}
	}
	dirs := make([]string, 0, len(groups))
	for dir := range groups {
		dirs = append(dirs, dir)
	}
	sort.Strings(dirs)
	for _, dir := range dirs {
		declarations := r.importDeclarations(dir, groups[dir])
		for _, s := range groups[dir] {
			if declaration := declarations[strings.Join(s.uses, ",")]; declaration != "" {
				s.text = s.form(declaration)
			}
		}
	}
}

// regionResult adds to the result the fragments of the fences in the host's
// coordinates, one fragment per host file.
func (r *run) regionResult(result sdk.IndexResult, snippets []*snippet) sdk.IndexResult {
	merged := map[string]*sdk.Fragment{}
	var untyped []string
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
		m := remap{text: s.text, lines: lineStarts(s.text.text), host: lineStarts([]byte(s.host.Text)), module: fragment.Modules[0].Path, shared: s.shared}
		mergeFragment(merged, m.fragment(fragment, s.host.Path), s.host.Path)
	}
	if len(untyped) > 0 {
		result.Notices = append(result.Notices, fmt.Sprintf(
			"%d Go fence(s) do not type-check (names the prose defines, or code that is not meant to compile); what resolves is reported, e.g. %s",
			len(untyped), untyped[0]))
	}
	for _, f := range merged {
		result.Fragments = append(result.Fragments, *f)
	}
	sort.Slice(result.Fragments, func(i, j int) bool { return result.Fragments[i].File.Path < result.Fragments[j].File.Path })
	return result
}

// importDeclarations is the import declaration of each set of uses of the
// fences, by the uses joined with commas. A name that the standard library or
// the module holds once, or the root package of a module it requires, is
// settled at once. The names a requirement may have further down are asked of
// goimports, which scans the module and its dependencies for every call, so
// the uses of all the fences go in one file; when the names of its answer are
// not the ones the uses make, each set is asked for on its own. A name no one
// of these knows is left unresolved: prose defines names, and a fence that
// uses them is a fragment.
func (r *run) importDeclarations(dir string, fences []*snippet) map[string]string {
	sets := map[string]*snippet{}
	union := map[string]bool{}
	names := map[string]bool{}
	for _, s := range fences {
		sets[strings.Join(s.uses, ",")] = s
		for _, use := range s.uses {
			union[use] = true
			names[packageOf(use)] = true
		}
	}
	byName, pending, pendingNames := r.fromTables(dir, union, names)
	asked, ok := ask(dir, pending, pendingNames)
	for name, spec := range asked {
		byName[name] = spec
	}
	out := map[string]string{}
	for key, s := range sets {
		if !ok {
			out[key] = declaration(goimports(dir, s.text.text))
			continue
		}
		var specs []importSpec
		seen := map[string]bool{}
		for _, use := range s.uses {
			name := packageOf(use)
			if spec, found := byName[name]; found && !seen[name] {
				seen[name] = true
				specs = append(specs, spec)
			}
		}
		out[key] = declaration(specs)
	}
	return out
}

// fromTables settles the names that the standard library, the module or a
// requirement's root package has once, and returns the uses and names that
// only goimports can settle.
func (r *run) fromTables(dir string, union, names map[string]bool) (byName map[string]importSpec, pending, pendingNames map[string]bool) {
	byName = map[string]importSpec{}
	pending = map[string]bool{}
	pendingNames = map[string]bool{}
	held := r.knownPackages(dir)
	for name := range names {
		if candidates := held[name]; len(candidates) == 1 {
			byName[name] = candidates[0]
		}
	}
	for use := range union {
		if _, settled := byName[packageOf(use)]; !settled {
			pending[use] = true
			pendingNames[packageOf(use)] = true
		}
	}
	if len(pending) == 0 {
		return byName, pending, pendingNames
	}
	settled, plausible := r.required(dir, pendingNames)
	for name, spec := range settled {
		byName[name] = spec
	}
	for name := range pendingNames {
		if len(held[name]) > 1 {
			plausible[name] = true
		}
	}
	for use := range pending {
		if !plausible[packageOf(use)] {
			delete(pending, use)
		}
	}
	for name := range pendingNames {
		if !plausible[name] {
			delete(pendingNames, name)
		}
	}
	return byName, pending, pendingNames
}

// ask has goimports settle the uses in one file; false when the names of its
// answer are not the ones the uses make.
func ask(dir string, pending, pendingNames map[string]bool) (map[string]importSpec, bool) {
	if len(pending) == 0 {
		return nil, true
	}
	return importsByName(goimports(dir, probe(pending)), pendingNames)
}

// packageOf is the package name of a use, `name.Selected`.
func packageOf(use string) string {
	name, _, _ := strings.Cut(use, ".")
	return name
}

// probe is a file that makes each of the uses.
func probe(uses map[string]bool) []byte {
	sorted := make([]string, 0, len(uses))
	for use := range uses {
		sorted = append(sorted, use)
	}
	sort.Strings(sorted)
	var b strings.Builder
	b.WriteString("package p\n\nvar (\n")
	for _, use := range sorted {
		b.WriteString("\t_ = " + use + "\n")
	}
	b.WriteString(")\n")
	return []byte(b.String())
}

// importsByName reads the imports by the name the file uses them under. It is
// false when that name is not the one the spec shows (the package clause of
// the imported package says it, and goimports does not), or when two imports
// have one name; the imports are then asked for set by set.
func importsByName(specs []importSpec, names map[string]bool) (map[string]importSpec, bool) {
	byName := map[string]importSpec{}
	for _, spec := range specs {
		name := spec.name
		if name == "" {
			name = importName(spec.path)
		}
		if _, dup := byName[name]; dup || !names[name] {
			return nil, false
		}
		byName[name] = spec
	}
	return byName, true
}

// importName is the name a file uses the package of an import path under, as
// the path shows it: the last element, the one before it for a major version,
// and none of a `.vN` suffix.
func importName(importPath string) string {
	name := path.Base(importPath)
	if i := strings.LastIndex(name, "."); i > 0 && isMajor(name[i+1:]) {
		name = name[:i]
	}
	if isMajor(name) {
		name = path.Base(path.Dir(importPath))
	}
	return name
}

// isMajor reports whether the element is a major version suffix, v2 and up.
func isMajor(element string) bool {
	if len(element) < 2 || element[0] != 'v' {
		return false
	}
	for _, c := range element[1:] {
		if c < '0' || c > '9' {
			return false
		}
	}
	return true
}

// declaration is the import declaration of the specs, empty for none.
func declaration(specs []importSpec) string {
	if len(specs) == 0 {
		return ""
	}
	var b strings.Builder
	b.WriteString("import (\n")
	for _, spec := range specs {
		b.WriteString("\t")
		if spec.name != "" {
			b.WriteString(spec.name + " ")
		}
		b.WriteString(strconv.Quote(spec.path) + "\n")
	}
	b.WriteString(")\n\n")
	return b.String()
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
	into.Comments = append(into.Comments, f.Comments...)
}

// goimports asks the vendor tool which imports the text lacks, as its module
// has them.
func goimports(dir string, text []byte) []importSpec {
	cwd, err := os.Getwd()
	if err != nil || os.Chdir(dir) != nil {
		return nil
	}
	defer func() { _ = os.Chdir(cwd) }()
	out, err := imports.Process(filepath.Join(dir, "lighthouse_snippet.go"), text, &imports.Options{Comments: true, TabIndent: true, TabWidth: 8})
	if err != nil {
		return nil
	}
	added, err := parser.ParseFile(token.NewFileSet(), "", out, parser.ImportsOnly)
	if err != nil {
		return nil
	}
	specs := make([]importSpec, 0, len(added.Imports))
	for _, spec := range added.Imports {
		path, err := strconv.Unquote(spec.Path.Value)
		if err != nil {
			continue
		}
		next := importSpec{path: path}
		if spec.Name != nil {
			next.name = spec.Name.Name
		}
		specs = append(specs, next)
	}
	return specs
}

// qualifiers are the uses of packages the file makes without importing, as
// `name.Selected`: what is selected from an identifier no declaration binds.
func qualifiers(file *ast.File) []string {
	unresolved := map[*ast.Ident]bool{}
	for _, id := range file.Unresolved {
		unresolved[id] = true
	}
	found := map[string]bool{}
	ast.Inspect(file, func(n ast.Node) bool {
		if sel, ok := n.(*ast.SelectorExpr); ok {
			if id, ok := sel.X.(*ast.Ident); ok && unresolved[id] {
				found[id.Name+"."+sel.Sel.Name] = true
			}
		}
		return true
	})
	uses := make([]string, 0, len(found))
	for use := range found {
		uses = append(uses, use)
	}
	sort.Strings(uses)
	return uses
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
