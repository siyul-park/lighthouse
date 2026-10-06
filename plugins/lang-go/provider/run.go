package provider

import (
	"fmt"
	"go/token"
	"go/types"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"strconv"
	"strings"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
	"golang.org/x/mod/modfile"
	"golang.org/x/tools/go/packages"
)

// run is the state of one index request.
type run struct {
	root string
	opts options

	requested map[string]sdk.FileRef
	fragments map[string]*sdk.Fragment
	claimed   map[string]bool
	// problems holds one reason per file that could not be fully analyzed.
	problems        map[string]string
	excludedFiles   map[string]bool
	excluded        int
	excludedExample string
	result          sdk.IndexResult
}

// batch is the set of files that one go.mod governs. Files with no go.mod form
// one synthetic batch rooted at the project root.
type batch struct {
	dir       string
	synthetic bool
	files     []string
}

type declaredType struct {
	obj  *types.TypeName
	unit *unit
}

const loadMode = packages.NeedName | packages.NeedFiles | packages.NeedCompiledGoFiles |
	packages.NeedImports | packages.NeedSyntax | packages.NeedTypes | packages.NeedTypesInfo

// syntheticModule names the module assumed for Go files with no go.mod.
const syntheticModule = "lighthouse.invalid/workspace"

func newRun(params sdk.IndexParams, opts options) *run {
	r := &run{
		root:          params.Project.Root,
		opts:          opts,
		requested:     map[string]sdk.FileRef{},
		fragments:     map[string]*sdk.Fragment{},
		claimed:       map[string]bool{},
		problems:      map[string]string{},
		excludedFiles: map[string]bool{},
		result:        emptyResult(),
	}
	for _, f := range params.Files {
		r.requested[f.Path] = f
	}
	return r
}

func (r *run) index() {
	var candidates []string
	for rel := range r.requested {
		if ignoredPath(rel) {
			r.fileOnly(rel)
			continue
		}
		candidates = append(candidates, rel)
	}
	sort.Strings(candidates)
	for _, b := range r.batches(candidates) {
		r.analyze(b)
	}
	for _, rel := range candidates {
		if !r.claimed[rel] {
			r.unclaimed(rel)
		}
	}
}

func (r *run) finish() sdk.IndexResult {
	paths := make([]string, 0, len(r.fragments))
	for path := range r.fragments {
		paths = append(paths, path)
	}
	sort.Strings(paths)
	for _, path := range paths {
		r.result.Fragments = append(r.result.Fragments, *r.fragments[path])
	}
	for path, reason := range r.problems {
		r.result.Incomplete = append(r.result.Incomplete, sdk.Incomplete{Path: path, Reason: reason})
	}
	sort.Slice(r.result.Incomplete, func(i, j int) bool {
		a, b := r.result.Incomplete[i], r.result.Incomplete[j]
		return a.Path < b.Path || (a.Path == b.Path && a.Reason < b.Reason)
	})
	if r.excluded > 0 {
		r.result.Notices = append(r.result.Notices, fmt.Sprintf(
			"%d file(s) excluded by build constraints, e.g. %s; select another context with [languages.go] tags or env",
			r.excluded, r.excludedExample))
	}
	return r.result
}

// batches assigns files to the nearest go.mod at or above their directory.
func (r *run) batches(files []string) []*batch {
	byDir := map[string]*batch{}
	cache := map[string]string{}
	for _, rel := range files {
		dir := filepath.Dir(filepath.Join(r.root, filepath.FromSlash(rel)))
		mod, ok := nearestModule(dir, cache)
		synthetic := !ok
		if synthetic {
			mod = r.root
		}
		b := byDir[mod]
		if b == nil {
			b = &batch{dir: mod, synthetic: synthetic}
			byDir[mod] = b
		}
		b.files = append(b.files, rel)
	}
	out := make([]*batch, 0, len(byDir))
	for _, b := range byDir {
		out = append(out, b)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].dir < out[j].dir })
	return out
}

func (r *run) analyze(b *batch) {
	pkgs, ok := r.load(b)
	if !ok {
		return
	}
	res := newResolver(r.root, pkgs, r.localModule(b))
	r.collectIgnored(pkgs)
	r.implements(r.claim(pkgs, res))
	r.packageErrors(pkgs)
}

// load type-checks the packages of b, recording why when the go command
// fails.
func (r *run) load(b *batch) ([]*packages.Package, bool) {
	cfg := &packages.Config{
		Mode:       loadMode,
		Dir:        b.dir,
		Env:        r.opts.environ(),
		BuildFlags: r.opts.buildFlags(),
		Tests:      true,
		Fset:       token.NewFileSet(),
	}
	if b.synthetic {
		cfg.Overlay = map[string][]byte{
			filepath.Join(b.dir, "go.mod"): []byte("module " + syntheticModule + "\n\ngo 1.21\n"),
		}
	}
	var pkgs []*packages.Package
	var err error
	r.opts.load(func() { pkgs, err = packages.Load(cfg, b.patterns(r.root)...) })
	if err == nil {
		return pkgs, true
	}
	dir, inside := r.rel(b.dir)
	if !inside {
		dir = b.dir
	}
	r.result.Incomplete = append(r.result.Incomplete, sdk.Incomplete{
		Reason: fmt.Sprintf("go list failed in %s, %d file(s) not analyzed: %v", dir, len(b.files), err),
	})
	for _, rel := range b.files {
		r.fileOnly(rel)
		r.problems[rel] = "go list failed"
	}
	return nil, false
}

// localModule reads the module path of the batch's go.mod.
func (r *run) localModule(b *batch) *goModule {
	dir, ok := r.rel(b.dir)
	if !ok {
		return nil
	}
	if dir == "" {
		dir = "."
	}
	if b.synthetic {
		return &goModule{path: syntheticModule, dir: dir}
	}
	data, err := os.ReadFile(filepath.Join(b.dir, "go.mod"))
	if err != nil {
		return nil
	}
	file, err := modfile.ParseLax("go.mod", data, nil)
	if err != nil || file.Module == nil {
		return nil
	}
	return &goModule{path: file.Module.Mod.Path, dir: dir}
}

// claim builds the fragment of every requested file from the first package
// variant that type-checked it.
func (r *run) claim(pkgs []*packages.Package, res *resolver) []*unit {
	var units []*unit
	for _, pkg := range claimOrder(pkgs) {
		if pkg.TypesInfo == nil || pkg.Types == nil {
			continue
		}
		for _, file := range pkg.Syntax {
			rel, ok := r.rel(pkg.Fset.Position(file.Package).Filename)
			if !ok || r.claimed[rel] || !r.isRequested(rel) {
				continue
			}
			src, err := os.ReadFile(filepath.Join(r.root, filepath.FromSlash(rel)))
			if err != nil {
				continue
			}
			r.claimed[rel] = true
			u := newUnit(rel, file, pkg, src, res)
			r.fragments[rel] = u.fragment()
			units = append(units, u)
		}
	}
	return units
}

// implements adds an `implements` edge from every declared type to every
// declared interface it satisfies, by value or by pointer.
func (r *run) implements(units []*unit) {
	interfaces, concrete := r.declaredTypes(units)
	sort.Slice(concrete, func(i, j int) bool { return concrete[i].obj.Pos() < concrete[j].obj.Pos() })
	for _, t := range concrete {
		for _, i := range interfaces {
			iface := i.obj.Type().Underlying().(*types.Interface)
			if !types.Implements(t.obj.Type(), iface) && !types.Implements(types.NewPointer(t.obj.Type()), iface) {
				continue
			}
			frag := r.fragments[t.unit.rel]
			frag.Edges = append(frag.Edges, sdk.Edge{
				Kind:       edgeImplements,
				From:       sdk.Node{Symbol: symbolID(kindType, t.unit.module, t.obj.Name())},
				To:         target(t.unit.res.module(i.obj.Pkg()), i.obj.Name()),
				Resolution: resolutionSemantic,
			})
		}
	}
}

// declaredTypes splits the types the units declare into interfaces worth
// matching and concrete types. Interfaces of one method are skipped unless
// asked for: nearly every type satisfies them by accident.
func (r *run) declaredTypes(units []*unit) (interfaces, concrete []declaredType) {
	byFile := map[string]*unit{}
	for _, u := range units {
		byFile[u.rel] = u
	}
	seen := map[*types.Package]bool{}
	for _, u := range units {
		if seen[u.pkg.Types] {
			continue
		}
		seen[u.pkg.Types] = true
		for _, t := range r.typesOf(u, byFile) {
			iface, isInterface := t.obj.Type().Underlying().(*types.Interface)
			switch {
			case !isInterface:
				concrete = append(concrete, t)
			case iface.NumMethods() > 1 || (iface.NumMethods() == 1 && r.opts.SmallInterfaces):
				interfaces = append(interfaces, t)
			}
		}
	}
	return interfaces, concrete
}

// typesOf lists the non-generic, non-alias types of u's package that are
// declared in a unit.
func (r *run) typesOf(u *unit, byFile map[string]*unit) []declaredType {
	var out []declaredType
	scope := u.pkg.Types.Scope()
	for _, name := range scope.Names() {
		obj, ok := scope.Lookup(name).(*types.TypeName)
		if !ok || obj.IsAlias() {
			continue
		}
		rel, ok := r.rel(u.fset.Position(obj.Pos()).Filename)
		owner := byFile[rel]
		named, isNamed := obj.Type().(*types.Named)
		if !ok || owner == nil || !isNamed || named.TypeParams().Len() > 0 {
			continue
		}
		out = append(out, declaredType{obj, owner})
	}
	return out
}

// packageErrors records, per requested file, the first load, parse or type
// error that touched it. Files excluded by build constraints are no error, and
// the compiler output the go command attaches to a package that fails to build
// repeats its type errors, so it only counts when nothing else explains the
// failure.
func (r *run) packageErrors(pkgs []*packages.Package) {
	reasons := map[string][]string{}
	for _, pkg := range pkgs {
		for _, e := range explained(pkg.Errors) {
			if strings.Contains(e.Msg, "build constraints exclude all Go files") {
				continue
			}
			reason := describe(e)
			for _, rel := range r.errorFiles(pkg, e) {
				if !slices.Contains(reasons[rel], reason) {
					reasons[rel] = append(reasons[rel], reason)
				}
			}
		}
	}
	for rel, list := range reasons {
		r.problems[rel] = list[0]
		if len(list) > 1 {
			r.problems[rel] = fmt.Sprintf("%s (+%d more error(s))", list[0], len(list)-1)
		}
	}
}

// errorFiles are the requested files an error is about: the file of its
// position, else every requested file of the package.
func (r *run) errorFiles(pkg *packages.Package, e packages.Error) []string {
	if rel, ok := r.rel(positionFile(e.Pos)); ok && r.isRequested(rel) {
		return []string{rel}
	}
	var out []string
	for _, f := range slices.Concat(pkg.GoFiles, pkg.IgnoredFiles) {
		if rel, ok := r.rel(f); ok && r.isRequested(rel) {
			out = append(out, rel)
		}
	}
	return out
}

func (r *run) collectIgnored(pkgs []*packages.Package) {
	for _, pkg := range pkgs {
		for _, f := range pkg.IgnoredFiles {
			if rel, ok := r.rel(f); ok {
				r.excludedFiles[rel] = true
			}
		}
	}
}

// unclaimed explains a file no package type-checked.
func (r *run) unclaimed(rel string) {
	r.fileOnly(rel)
	if _, known := r.problems[rel]; known {
		return
	}
	if r.excludedFiles[rel] {
		r.excluded++
		if r.excludedExample == "" {
			r.excludedExample = rel
		}
		return
	}
	r.problems[rel] = "not part of any package the go command loaded"
}

func (r *run) fileOnly(rel string) {
	r.fragments[rel] = emptyFragment(sdk.FileInfo{Path: rel})
}

// rel is the project-relative, slash-separated form of an absolute path.
func (r *run) rel(abs string) (string, bool) {
	p, err := filepath.Rel(r.root, abs)
	if err != nil || p == ".." || strings.HasPrefix(p, ".."+string(filepath.Separator)) {
		return "", false
	}
	return filepath.ToSlash(p), true
}

func (r *run) isRequested(rel string) bool {
	_, ok := r.requested[rel]
	return ok
}

// patterns are the package directories of b's files, relative to its go.mod.
func (b *batch) patterns(root string) []string {
	dirs := map[string]bool{}
	for _, rel := range b.files {
		abs := filepath.Dir(filepath.Join(root, filepath.FromSlash(rel)))
		if p, err := filepath.Rel(b.dir, abs); err == nil {
			dirs[p] = true
		}
	}
	out := make([]string, 0, len(dirs))
	for d := range dirs {
		if d == "." {
			out = append(out, ".")
		} else {
			out = append(out, "./"+filepath.ToSlash(d))
		}
	}
	sort.Strings(out)
	return out
}

func nearestModule(dir string, cache map[string]string) (string, bool) {
	var visited []string
	found, ok := "", false
	for at := dir; ; at = filepath.Dir(at) {
		if hit, cached := cache[at]; cached {
			found, ok = hit, hit != ""
			break
		}
		visited = append(visited, at)
		if _, err := os.Stat(filepath.Join(at, "go.mod")); err == nil {
			found, ok = at, true
			break
		}
		if filepath.Dir(at) == at {
			break
		}
	}
	for _, v := range visited {
		cache[v] = found
	}
	return found, ok
}

// claimOrder puts external test packages first, then test variants, then
// plain packages, so each file is read from the variant that sees the most
// of it. The synthetic package of a test binary has no requested files.
func claimOrder(pkgs []*packages.Package) []*packages.Package {
	rank := func(p *packages.Package) int {
		switch {
		case isExternalTest(p):
			return 0
		case p.ForTest != "":
			return 1
		default:
			return 2
		}
	}
	out := slices.Clone(pkgs)
	sort.SliceStable(out, func(i, j int) bool { return rank(out[i]) < rank(out[j]) })
	return out
}

// explained drops compile output when a parse, type or other list error says
// why the package failed.
func explained(errs []packages.Error) []packages.Error {
	var source []packages.Error
	for _, e := range errs {
		if e.Kind != packages.ListError || !strings.HasPrefix(e.Msg, "# ") {
			source = append(source, e)
		}
	}
	if len(source) == 0 {
		return errs
	}
	return source
}

// describe is `line:col: message` for positioned errors, else the message,
// reduced to its first line.
func describe(e packages.Error) string {
	msg, _, _ := strings.Cut(e.Msg, "\n")
	file := positionFile(e.Pos)
	if file == "" || file == e.Pos {
		return msg
	}
	return strings.TrimPrefix(e.Pos, file+":") + ": " + msg
}

// positionFile strips the trailing :line:col of a position.
func positionFile(pos string) string {
	for i := 0; i < 2; i++ {
		at := strings.LastIndex(pos, ":")
		if at < 0 {
			return pos
		}
		if _, err := strconv.Atoi(pos[at+1:]); err != nil {
			return pos
		}
		pos = pos[:at]
	}
	return pos
}

// ignoredPath applies the go tool's rule for names it skips: testdata and
// vendor directories, and names starting with "." or "_".
func ignoredPath(rel string) bool {
	for _, part := range strings.Split(rel, "/") {
		if part == "testdata" || part == "vendor" || strings.HasPrefix(part, ".") || strings.HasPrefix(part, "_") {
			return true
		}
	}
	return false
}

func emptyResult() sdk.IndexResult {
	return sdk.IndexResult{
		Fragments:  []sdk.Fragment{},
		Notices:    []string{},
		Incomplete: []sdk.Incomplete{},
	}
}

func emptyFragment(file sdk.FileInfo) *sdk.Fragment {
	return &sdk.Fragment{
		File:      file,
		Modules:   []sdk.Module{},
		Symbols:   []sdk.Symbol{},
		Edges:     []sdk.Edge{},
		Functions: []sdk.FunctionSummary{},
		Tests:     []sdk.TestCase{},
		Comments:  []sdk.Comment{},
	}
}
