package provider

import (
	"encoding/json"
	"errors"
	"fmt"
	"go/token"
	"go/types"
	"os"
	"path/filepath"
	"runtime"
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

	// overlays hold the text that stands in for a file, by absolute path.
	overlays     map[string][]byte
	requested    map[string]sdk.FileRef
	astRequested map[string]bool
	fragments    map[string]*sdk.Fragment
	claimed      map[string]bool
	// problems holds one reason per file that could not be fully analyzed.
	problems        map[string]string
	excludedFiles   map[string]bool
	excluded        int
	excludedExample string
	result          sdk.IndexResult
	// failed counts, per batch the go command could not list, its files.
	failed map[string]int
	// encoded holds fragments that are already JSON, from the cache.
	encoded map[string]json.RawMessage
	// loads counts the batches the run had the go command load.
	loads int
	// chain is where the go command keeps its libraries, once read.
	chain *toolchain
	// fenceFiles are the files that stand for the fences of host files: they
	// are analysed with the rest but are no files of the request.
	fenceFiles map[string]bool
	// far holds, by file, the methods that implement only an interface of a
	// package of the project that the package of the file does not import.
	far map[string][]string
	// times is where the request's time goes, shared with the analysis of the
	// units the cache missed.
	times *timings
	// regionModules gives the module path of the unit of a fence by its file,
	// where the unit's directory is not its own.
	regionModules map[string]string
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

// loader loads the batches in the background, at most one load per CPU in
// flight or waiting to be read, and hands the results out in batch order, so
// that what is read from them does not depend on which load finished first.
type loader struct {
	ready []chan loaded
	slots chan struct{}
	read  int
}

// loaded is what loading the packages of one batch gave.
type loaded struct {
	pkgs []*packages.Package
	err  error
}

const loadMode = packages.NeedName | packages.NeedFiles | packages.NeedCompiledGoFiles |
	packages.NeedImports | packages.NeedSyntax | packages.NeedTypes | packages.NeedTypesInfo

// syntheticModule names the module assumed for Go files with no go.mod.
const syntheticModule = "lighthouse.invalid/workspace"

func newRun(params sdk.IndexParams, opts options) *run {
	r := &run{
		root:          params.Project.Root,
		opts:          opts,
		overlays:      map[string][]byte{},
		requested:     map[string]sdk.FileRef{},
		astRequested:  map[string]bool{},
		fragments:     map[string]*sdk.Fragment{},
		claimed:       map[string]bool{},
		problems:      map[string]string{},
		excludedFiles: map[string]bool{},
		failed:        map[string]int{},
		encoded:       map[string]json.RawMessage{},
		regionModules: map[string]string{},
		fenceFiles:    map[string]bool{},
		result:        emptyResult(),
		times:         &timings{},
		far:           map[string][]string{},
	}
	for _, f := range params.FileInputs() {
		r.requested[f.Path] = f
	}
	for _, input := range params.Inputs {
		if input.Kind == "file" && input.File != nil && input.SourceAST {
			r.astRequested[input.File.Path] = true
		}
	}
	for _, o := range params.Context.Overlays {
		r.overlays[filepath.Join(r.root, filepath.FromSlash(o.Path))] = []byte(o.Text)
	}
	return r
}

func (r *run) index() {
	candidates := r.candidates()
	batches := r.batches(candidates)
	r.loads = len(batches)
	loads := r.loadAll(batches)
	for _, b := range batches {
		waited := r.times.measure(phaseLoad)
		got := loads.next()
		waited()
		r.analyze(b, got)
	}
	for _, rel := range candidates {
		if !r.claimed[rel] {
			r.unclaimed(rel)
		}
	}
	r.excludedTests(candidates)
}

// edits are the overlays of a request by absolute path, when each stands in
// for the text of an existing Go file: a unit is keyed on what its files say,
// so the text of an edit is read where the disk's would be. An overlay that
// adds, empties or removes a file, or is not Go, changes what the directory
// holds and so what the units are: the request is then analyzed uncached.
func (r *run) edits(overlays []sdk.Overlay) (map[string][]byte, bool) {
	edits := make(map[string][]byte, len(overlays))
	for _, o := range overlays {
		path := filepath.Join(r.root, filepath.FromSlash(o.Path))
		info, err := os.Stat(path)
		if !strings.HasSuffix(o.Path, ".go") || o.Text == "" || err != nil || !info.Mode().IsRegular() {
			return nil, false
		}
		edits[path] = []byte(o.Text)
	}
	return edits, true
}

// candidates are the requested files, sorted: the host lists the files of the
// project, so every one is analyzed.
func (r *run) candidates() []string {
	candidates := make([]string, 0, len(r.requested))
	for rel := range r.requested {
		candidates = append(candidates, rel)
	}
	sort.Strings(candidates)
	return candidates
}

func (r *run) finish() sdk.IndexResult {
	defer r.times.measure(phaseSerialise)()
	paths := make([]string, 0, len(r.fragments))
	for path := range r.fragments {
		if !r.fenceFiles[path] {
			paths = append(paths, path)
		}
	}
	sort.Strings(paths)
	for _, path := range paths {
		r.result.Fragments = append(r.result.Fragments, *r.fragments[path])
	}
	encoded := make([]string, 0, len(r.encoded))
	for path := range r.encoded {
		encoded = append(encoded, path)
	}
	sort.Strings(encoded)
	for _, path := range encoded {
		r.result.Encoded = append(r.result.Encoded, sdk.EncodedFragment{Path: path, JSON: r.encoded[path]})
	}
	sort.Slice(r.result.Sources, func(i, j int) bool {
		return r.result.Sources[i].Path < r.result.Sources[j].Path
	})
	for path, reason := range r.problems {
		if r.fenceFiles[path] {
			continue
		}
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

// analyze reads the code model out of the packages that loading b gave.
func (r *run) analyze(b *batch, l loaded) {
	pkgs, ok := r.unpack(b, l)
	if !ok {
		return
	}
	defer r.times.measure(phaseAnalyse)()
	r.collectSources(pkgs)
	res := newResolver(r.root, pkgs, r.localModule(b))
	r.collectIgnored(pkgs)
	r.implements(r.claim(pkgs, res))
	r.packageErrors(pkgs)
}

// loadAll starts loading the packages of every batch. Batches share nothing
// but the go command's caches, so their loads are independent. The load of a
// run that sets the go binary edits this process's PATH (options.load), which
// concurrent loads would race on; those loads therefore all finish here, one
// after the other, before any analysis starts, and nothing else runs while
// PATH is edited.
func (r *run) loadAll(batches []*batch) *loader {
	l := &loader{ready: make([]chan loaded, len(batches)), slots: make(chan struct{}, runtime.GOMAXPROCS(0))}
	for i := range l.ready {
		l.ready[i] = make(chan loaded, 1)
	}
	if r.opts.Go != "" {
		defer r.times.measure(phaseLoad)()
		for i, b := range batches {
			pkgs, err := r.loadPackages(b)
			l.ready[i] <- loaded{pkgs: pkgs, err: err}
		}
		return l
	}
	go func() {
		for i, b := range batches {
			l.slots <- struct{}{}
			go func() {
				pkgs, err := r.loadPackages(b)
				l.ready[i] <- loaded{pkgs: pkgs, err: err}
			}()
		}
	}()
	return l
}

// unpack returns the packages of b, recording why when the go command failed.
func (r *run) unpack(b *batch, l loaded) ([]*packages.Package, bool) {
	if l.err == nil {
		return l.pkgs, true
	}
	dir, inside := r.rel(b.dir)
	if !inside {
		dir = b.dir
	}
	r.failed[b.dir] = len(b.files)
	r.result.Incomplete = append(r.result.Incomplete, sdk.Incomplete{
		Reason: fmt.Sprintf("go list failed in %s, %d file(s) not analyzed: %v", dir, len(b.files), l.err),
	})
	for _, rel := range b.files {
		r.fileOnly(rel)
		r.problems[rel] = "go list failed"
	}
	return nil, false
}

// loadPackages type-checks the packages of b. It reads the run and changes
// nothing in it.
func (r *run) loadPackages(b *batch) ([]*packages.Package, error) {
	cfg := &packages.Config{
		Mode:       loadMode,
		Dir:        b.dir,
		Env:        r.opts.environ(),
		BuildFlags: r.opts.buildFlags(),
		Tests:      true,
		Fset:       token.NewFileSet(),
	}
	cfg.Overlay = map[string][]byte{}
	for path, text := range r.overlays {
		cfg.Overlay[path] = text
	}
	if b.synthetic {
		cfg.Overlay[filepath.Join(b.dir, "go.mod")] = []byte("module " + syntheticModule + "\n\ngo 1.21\n")
	}
	var pkgs []*packages.Package
	var err error
	r.opts.load(func() { pkgs, err = packages.Load(cfg, b.patterns(r.root)...) })
	if err == nil && len(pkgs) == 0 && len(b.files) > 0 {
		// An environment the go command rejects, such as an unsupported
		// GOOS/GOARCH pair, lists nothing and says nothing to go/packages.
		err = errors.New("the go command listed no package; check GOOS, GOARCH, GOFLAGS and the tags and env of [languages.go]")
	}
	return pkgs, err
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
			src, ok := r.overlays[filepath.Join(r.root, filepath.FromSlash(rel))]
			if !ok {
				var err error
				src, err = os.ReadFile(filepath.Join(r.root, filepath.FromSlash(rel)))
				if err != nil {
					continue
				}
			}
			r.claimed[rel] = true
			u := newUnit(rel, file, pkg, src, res)
			u.frag.File.Facts = r.stemFacts(rel)
			u.fence = r.fenceFiles[rel]
			if module, ok := r.regionModules[rel]; ok {
				u.module = module
				u.frag.Modules[0].Path = module
			}
			r.fragments[rel] = u.fragment()
			r.far[rel] = u.far
			units = append(units, u)
		}
	}
	return units
}

// implements adds an `implements` edge from every declared type to every
// declared interface it satisfies, by value or by pointer.
func (r *run) implements(units []*unit) {
	interfaces, concrete := r.declaredTypes(units)
	sort.Slice(concrete, func(i, j int) bool { return declaredBefore(concrete[i], concrete[j]) })
	sort.Slice(interfaces, func(i, j int) bool { return declaredBefore(interfaces[i], interfaces[j]) })
	for _, t := range concrete {
		for _, i := range interfaces {
			iface := i.obj.Type().Underlying().(*types.Interface)
			if !satisfies(t.obj.Type().(*types.Named), iface) {
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
			case isGeneric(t.obj):
				// a generic interface is a constraint family, not a method set to match
			case iface.NumMethods() > 1 || (iface.NumMethods() == 1 && r.opts.SmallInterfaces):
				interfaces = append(interfaces, t)
			}
		}
	}
	return interfaces, concrete
}

// typesOf lists the non-alias types of u's package that are
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
		_, isNamed := obj.Type().(*types.Named)
		if !ok || owner == nil || !isNamed {
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
	delete(r.encoded, rel)
	r.fragments[rel] = emptyFragment(sdk.FileInfo{Path: rel, Facts: r.stemFacts(rel)})
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

// next waits for the load of the next batch in order and frees its slot.
func (l *loader) next() loaded {
	got := <-l.ready[l.read]
	l.read++
	select {
	case <-l.slots:
	default: // loaded up front, no slot was taken
	}
	return got
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

func emptyResult() sdk.IndexResult {
	return sdk.IndexResult{
		Fragments:  []sdk.Fragment{},
		Sources:    []sdk.Source{},
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

// declaredBefore orders types by where they are declared, which does not
// depend on the order in which the go command's packages were parsed.
func declaredBefore(a, b declaredType) bool {
	pa, pb := a.unit.fset.PositionFor(a.obj.Pos(), false), b.unit.fset.PositionFor(b.obj.Pos(), false)
	if pa.Filename != pb.Filename {
		return pa.Filename < pb.Filename
	}
	if pa.Offset != pb.Offset {
		return pa.Offset < pb.Offset
	}
	return a.obj.Id() < b.obj.Id()
}

func isGeneric(obj *types.TypeName) bool {
	named, ok := obj.Type().(*types.Named)
	return ok && named.TypeParams().Len() > 0
}
