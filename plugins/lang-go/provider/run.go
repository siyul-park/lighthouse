package provider

import (
	"fmt"
	"go/token"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
	"golang.org/x/mod/modfile"
	"golang.org/x/tools/go/packages"
)

const loadMode = packages.NeedName | packages.NeedFiles | packages.NeedCompiledGoFiles |
	packages.NeedImports | packages.NeedSyntax | packages.NeedTypes | packages.NeedTypesInfo

// syntheticModule names the module assumed for Go files with no go.mod.
const syntheticModule = "lighthouse.invalid/workspace"

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

func newRun(params sdk.IndexParams) *run {
	r := &run{
		root:      params.Project.Root,
		requested: map[string]sdk.FileRef{},
		fragments: map[string]*sdk.Fragment{},
		claimed:   map[string]bool{},
		problems:  map[string]string{},
		result: sdk.IndexResult{
			Fragments:  []sdk.Fragment{},
			Notices:    []string{},
			Incomplete: []sdk.Incomplete{},
		},
	}
	for _, f := range params.Files {
		r.requested[f.Path] = f
	}
	return r
}

// group is the set of files that one go.mod governs.
type group struct {
	dir       string
	synthetic bool
	files     []string
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
	for _, g := range r.groups(candidates) {
		r.analyze(g)
	}
	for _, rel := range candidates {
		if !r.claimed[rel] {
			r.unclaimed(rel)
		}
	}
}

func (r *run) fileOnly(rel string) {
	r.fragments[rel] = &sdk.Fragment{
		File:      sdk.FileInfo{Path: rel},
		Modules:   []sdk.Module{},
		Symbols:   []sdk.Symbol{},
		Edges:     []sdk.Edge{},
		Functions: []sdk.FunctionSummary{},
		Tests:     []sdk.TestCase{},
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

// groups assigns files to the nearest go.mod at or above their directory;
// files with none form one synthetic module rooted at the project root.
func (r *run) groups(files []string) []*group {
	byDir := map[string]*group{}
	cache := map[string]string{}
	for _, rel := range files {
		dir := filepath.Dir(filepath.Join(r.root, filepath.FromSlash(rel)))
		mod, ok := nearestModule(dir, cache)
		synthetic := !ok
		if synthetic {
			mod = r.root
		}
		g := byDir[mod]
		if g == nil {
			g = &group{dir: mod, synthetic: synthetic}
			byDir[mod] = g
		}
		g.files = append(g.files, rel)
	}
	out := make([]*group, 0, len(byDir))
	for _, g := range byDir {
		out = append(out, g)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].dir < out[j].dir })
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

func (r *run) patterns(g *group) []string {
	dirs := map[string]bool{}
	for _, rel := range g.files {
		abs := filepath.Dir(filepath.Join(r.root, filepath.FromSlash(rel)))
		if p, err := filepath.Rel(g.dir, abs); err == nil {
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

func (r *run) analyze(g *group) {
	cfg := &packages.Config{
		Mode:       loadMode,
		Dir:        g.dir,
		Env:        r.opts.environ(),
		BuildFlags: r.opts.buildFlags(),
		Tests:      true,
		Fset:       token.NewFileSet(),
	}
	if g.synthetic {
		cfg.Overlay = map[string][]byte{
			filepath.Join(g.dir, "go.mod"): []byte("module " + syntheticModule + "\n\ngo 1.21\n"),
		}
	}
	var pkgs []*packages.Package
	var err error
	r.opts.load(func() { pkgs, err = packages.Load(cfg, r.patterns(g)...) })
	if err != nil {
		r.result.Incomplete = append(r.result.Incomplete, sdk.Incomplete{
			Reason: fmt.Sprintf("go list failed in %s, %d file(s) not analyzed: %v", r.display(g.dir), len(g.files), err),
		})
		for _, rel := range g.files {
			r.fileOnly(rel)
			r.problems[rel] = "go list failed"
		}
		return
	}
	res := newResolver(r.root, pkgs, r.moduleOf(g))
	r.collectIgnored(pkgs)
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
			frag := buildFragment(u)
			r.fragments[rel] = &frag
			units = append(units, u)
		}
	}
	r.implements(units, res)
	r.packageErrors(pkgs)
}

// moduleOf reads the module path of the group's go.mod.
func (r *run) moduleOf(g *group) *goModule {
	dir, ok := r.rel(g.dir)
	if !ok {
		return nil
	}
	if dir == "" {
		dir = "."
	}
	if g.synthetic {
		return &goModule{path: syntheticModule, dir: dir}
	}
	data, err := os.ReadFile(filepath.Join(g.dir, "go.mod"))
	if err != nil {
		return nil
	}
	file, err := modfile.ParseLax("go.mod", data, nil)
	if err != nil || file.Module == nil {
		return nil
	}
	return &goModule{path: file.Module.Mod.Path, dir: dir}
}

func (r *run) isRequested(rel string) bool {
	_, ok := r.requested[rel]
	return ok
}

func (r *run) display(abs string) string {
	if rel, ok := r.rel(abs); ok {
		return rel
	}
	return abs
}

// rel is the project-relative, slash-separated form of an absolute path.
func (r *run) rel(abs string) (string, bool) {
	p, err := filepath.Rel(r.root, abs)
	if err != nil || p == ".." || strings.HasPrefix(p, ".."+string(filepath.Separator)) {
		return "", false
	}
	return filepath.ToSlash(p), true
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
	out := append([]*packages.Package(nil), pkgs...)
	sort.SliceStable(out, func(i, j int) bool { return rank(out[i]) < rank(out[j]) })
	return out
}

func (r *run) collectIgnored(pkgs []*packages.Package) {
	if r.excludedFiles == nil {
		r.excludedFiles = map[string]bool{}
	}
	for _, pkg := range pkgs {
		for _, f := range pkg.IgnoredFiles {
			if rel, ok := r.rel(f); ok {
				r.excludedFiles[rel] = true
			}
		}
	}
}

// packageErrors records, per requested file, the first load, parse or type
// error that touched it. Files excluded by build constraints are no error, and
// the compiler output the go command attaches to a package that fails to build
// repeats its type errors, so it only counts when nothing else explains the
// failure.
func (r *run) packageErrors(pkgs []*packages.Package) {
	type key struct{ file, msg string }
	counts := map[string]int{}
	seen := map[key]bool{}
	for _, pkg := range pkgs {
		for _, e := range explained(pkg.Errors) {
			if strings.Contains(e.Msg, "build constraints exclude all Go files") {
				continue
			}
			reason := describe(e)
			for _, rel := range r.errorFiles(pkg, e) {
				if seen[key{rel, reason}] {
					continue
				}
				seen[key{rel, reason}] = true
				counts[rel]++
				if counts[rel] == 1 {
					r.problems[rel] = reason
				}
			}
		}
	}
	for rel, n := range counts {
		if n > 1 {
			r.problems[rel] = fmt.Sprintf("%s (+%d more error(s))", r.problems[rel], n-1)
		}
	}
}

func compileOutput(e packages.Error) bool {
	return e.Kind == packages.ListError && strings.HasPrefix(e.Msg, "# ")
}

// explained drops compile output when a parse, type or other list error says
// why the package failed.
func explained(errs []packages.Error) []packages.Error {
	hasSource := false
	for _, e := range errs {
		hasSource = hasSource || !compileOutput(e)
	}
	if !hasSource {
		return errs
	}
	var out []packages.Error
	for _, e := range errs {
		if !compileOutput(e) {
			out = append(out, e)
		}
	}
	return out
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

// errorFiles are the requested files an error is about: the file of its
// position, else every requested file of the package.
func (r *run) errorFiles(pkg *packages.Package, e packages.Error) []string {
	if rel, ok := r.rel(positionFile(e.Pos)); ok && r.isRequested(rel) {
		return []string{rel}
	}
	var out []string
	for _, f := range append(append([]string(nil), pkg.GoFiles...), pkg.IgnoredFiles...) {
		if rel, ok := r.rel(f); ok && r.isRequested(rel) {
			out = append(out, rel)
		}
	}
	return out
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
