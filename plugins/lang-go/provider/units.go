package provider

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"go/types"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"strconv"
	"strings"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

// shape is how a request is analyzed: which directories are asked for, which
// are loaded with them, and the `api` of each of those asked for.
type shape struct {
	pl *planner
	// asked are the directories of the units of files, by the host's id.
	asked map[string]*directory
	// interfaces are the directories that declare interfaces.
	interfaces []*directory
}

// planner reads the directories of the project and tells what the host needs
// to know of each: its `api`, the directories it imports, and whether it
// declares interfaces.
type planner struct {
	r       *run
	modules []moduleRoot
	dirs    map[string]*directory
	// reads are the files outside the project the answers depend on.
	reads []string
}

// directory is a package directory, the unit of the host. Its `api` covers its
// own files without the bodies of their functions and what it imports,
// directly or not; an edit inside a body leaves it as it was.
type directory struct {
	rel, abs string
	// requested are the project-relative files the host asked for, sorted.
	requested []string
	content   string
	api       string
	ifaces    string
	imports   []string
	deps      []*directory
	// matching tells which interfaces can match the types of the directory,
	// and which types can match the interfaces of the others.
	matching matching
}

// matching is what decides, from text alone, whether the types of one
// directory can implement an interface of another: a type implements an
// interface only if it has a method of each name of the interface. It errs
// towards "can": a name set that is not known is an open one.
type matching struct {
	// methods are the names of the methods the files declare.
	methods map[string]bool
	// open is set when the method sets of the types are not the declared
	// methods alone (an embedded field promotes more), or are not known.
	open bool
	// interfaces are the explicit method names of each interface the files
	// declare, one list for each; unknown when a type may stand for one that
	// the text does not show.
	interfaces [][]string
	unknown    bool
}

// moduleRoot is a module that holds project code: its import path and the
// directory it lives in.
type moduleRoot struct{ path, dir string }

// dirDigest collects what the files of one directory contribute to its unit.
type dirDigest struct {
	content, api, ifaces, imports []string // ifaces holds only the non-empty
	matching                      matching
}

// fileFacts are what the host needs to know about one Go source file, derived
// from its text alone.
type fileFacts struct {
	// Imports are the import paths of the file.
	Imports []string
	// API digests the text of the file without the bodies of its functions and
	// methods. An edit inside a body leaves it unchanged, and with it the `api`
	// of every package that depends on this one (early cutoff).
	API string
	// Interfaces digests the type declarations that may declare an interface;
	// empty when there are none. `implements` edges read every project
	// interface, so a unit that declares one is imported by all.
	Interfaces string
	matching   matching
}

// goEnvVars are the settings of the go command that decide which files make a
// package and what they mean: the toolchain, the target, the flags and the
// workspace. The host puts their values in the key of every unit.
var goEnvVars = []string{
	"GOROOT", "GOOS", "GOARCH", "GOFLAGS", "GOEXPERIMENT", "CGO_ENABLED", "GOWORK",
	"GOTOOLCHAIN", "GOAMD64", "GOARM", "GOARM64", "GO386", "GOMIPS", "GOMIPS64", "GOPPC64", "GORISCV64", "GOFIPS140",
}

// importsOf are the directories d imports and the ones that declare
// interfaces, which every package reads for its `implements` edges, sorted.
func (s *shape) importsOf(d *directory) []string {
	seen := map[string]bool{}
	for _, dep := range d.deps {
		seen[dep.rel] = true
	}
	for _, iface := range s.interfaces {
		seen[iface.rel] = true
	}
	delete(seen, d.rel)
	out := make([]string, 0, len(seen))
	for rel := range seen {
		if !filepath.IsAbs(rel) {
			out = append(out, rel)
		}
	}
	sort.Strings(out)
	return out
}

// moduleRoots are the modules of the project, one for each batch: what turns
// an import path into a directory.
func (pl *planner) moduleRoots(batches []*batch) bool {
	for _, b := range batches {
		local := pl.r.localModule(b)
		if local == nil {
			return false
		}
		pl.modules = append(pl.modules, moduleRoot{local.path, b.dir})
	}
	return true
}

// unit returns the directory at an absolute path, reading it once. A directory
// without Go files is no unit.
func (pl *planner) unit(abs string) *directory {
	if u, known := pl.dirs[abs]; known {
		return u
	}
	u := pl.read(abs)
	pl.dirs[abs] = u
	if u != nil {
		pl.link(u)
	}
	return u
}

func (pl *planner) read(abs string) *directory {
	entries, err := os.ReadDir(abs)
	if err != nil {
		return nil
	}
	rel, inside := pl.r.rel(abs)
	if !inside {
		rel = abs
	}
	u := &directory{rel: rel, abs: abs}
	var d dirDigest
	for _, e := range entries {
		if !pl.entry(u, &d, e) {
			return nil
		}
	}
	if len(d.content) == 0 {
		return nil
	}
	d.seal(u)
	u.matching = d.matching
	return u
}

// entry adds one directory entry to the digest of u; false when it cannot be read.
func (pl *planner) entry(u *directory, d *dirDigest, e os.DirEntry) bool {
	name := e.Name()
	switch {
	case e.IsDir():
		return true
	case strings.HasSuffix(name, ".go"):
		return pl.goFile(u, d, name)
	default:
		return true
	}
}

func (pl *planner) goFile(u *directory, d *dirDigest, name string) bool {
	rel := joinRel(u.rel, name)
	facts, hash, ok := pl.factsOfFile(u.abs, rel)
	if !ok {
		return false
	}
	if _, requested := pl.r.requested[rel]; requested {
		u.requested = append(u.requested, rel)
	}
	d.content = append(d.content, name, hash)
	d.api = append(d.api, name, facts.API)
	if facts.Interfaces != "" {
		d.ifaces = append(d.ifaces, name, facts.Interfaces)
	}
	d.matching.add(facts.matching)
	d.imports = append(d.imports, facts.Imports...)
	return true
}

// factsOfFile returns the facts and content hash of a file, read from its
// overlay when it has one.
func (pl *planner) factsOfFile(abs, rel string) (fileFacts, string, bool) {
	path := filepath.Join(abs, filepath.Base(rel))
	src, edited := pl.r.overlays[path]
	if !edited {
		var err error
		if src, err = os.ReadFile(path); err != nil {
			return fileFacts{}, "", false
		}
	}
	return factsOf(src), contentHash(src), true
}

// link resolves the imports of u into units.
func (pl *planner) link(u *directory) {
	for _, path := range u.imports {
		dir, ok := pl.importDir(path)
		if !ok {
			continue
		}
		if dep := pl.unit(dir); dep != nil && dep != u {
			u.deps = append(u.deps, dep)
		}
	}
}

// importDir is the directory of an import path inside a project module.
func (pl *planner) importDir(path string) (string, bool) {
	best := -1
	for i, m := range pl.modules {
		rest, ok := strings.CutPrefix(path, m.path)
		if ok && (rest == "" || rest[0] == '/') && (best < 0 || len(m.path) > len(pl.modules[best].path)) {
			best = i
		}
	}
	if best < 0 {
		return "", false
	}
	m := pl.modules[best]
	return filepath.Join(m.dir, filepath.FromSlash(strings.TrimPrefix(path, m.path))), true
}

// seal sets the digests of u from what its files contributed.
func (d *dirDigest) seal(u *directory) {
	sort.Strings(u.requested)
	slices.Sort(d.imports)
	u.imports = slices.Compact(d.imports)
	u.content = digest(d.content...)
	u.api = digest(d.api...)
	if len(d.ifaces) > 0 {
		u.ifaces = digest(append([]string{u.rel}, d.ifaces...)...)
	}
}

// plan reads every directory of the project and has the run load, besides the
// units asked for, the directories that declare an interface the types of the
// units can implement: a method is an implementation of any interface of the
// project, so types are matched against all of them. A directory whose
// interfaces list a method that no type of the units has is not loaded: loading
// one makes the go command build every package it imports that an edit changed.
func (r *run) plan(params sdk.IndexParams) *shape {
	pl := &planner{r: r, dirs: map[string]*directory{}}
	files := map[string][]string{}
	for _, rel := range params.Files {
		abs := filepath.Dir(filepath.Join(r.root, filepath.FromSlash(rel)))
		files[abs] = append(files[abs], rel)
	}
	s := &shape{pl: pl, asked: map[string]*directory{}}
	batches := r.batches(params.Files)
	if !pl.moduleRoots(batches) {
		return s
	}
	pl.outside(batches)
	mine := matching{}
	for _, u := range params.Units {
		if len(u.Files) > 0 {
			d := pl.unit(filepath.Join(r.root, filepath.FromSlash(u.ID)))
			s.asked[u.ID] = d
			if d != nil {
				mine.add(d.matching)
			}
		}
		// The fences of host files are matched against every interface.
		mine.open = mine.open || len(u.Regions) > 0
	}
	dirs := make([]string, 0, len(files))
	for abs := range files {
		dirs = append(dirs, abs)
	}
	sort.Strings(dirs)
	for _, abs := range dirs {
		d := pl.unit(abs)
		if d == nil || d.ifaces == "" {
			continue
		}
		s.interfaces = append(s.interfaces, d)
		if !mine.mayMatch(d.matching) {
			continue
		}
		for _, rel := range files[abs] {
			if _, asked := r.requested[rel]; !asked {
				r.requested[rel] = sdk.FileRef{Path: rel}
			}
		}
	}
	return s
}

// unitResults splits what the run found into the answers of the units asked
// for: the files of a directory and the regions of the host files.
func (r *run) unitResults(s *shape, params sdk.IndexParams, snippets []*snippet, found answer) []sdk.UnitResult {
	var out []sdk.UnitResult
	for _, u := range params.Units {
		if len(u.Files) > 0 {
			out = append(out, r.directoryResult(s, u, found))
		}
	}
	for _, u := range params.Units {
		if len(u.Regions) > 0 {
			out = append(out, r.regionsResult(s, u, snippets))
		}
	}
	return out
}

// directoryResult is the answer for one directory: its fragments, trees and
// gaps, and the build-constraint notice of its files.
func (r *run) directoryResult(s *shape, u sdk.UnitRef, found answer) sdk.UnitResult {
	own := map[string]bool{}
	hashes := make([]string, 0, len(u.Files))
	for _, f := range u.Files {
		own[f.Path] = true
		hashes = append(hashes, f.Path, f.Hash)
	}
	res := sdk.UnitResult{ID: u.ID, Fragments: []sdk.Fragment{}, Notices: []string{}, Incomplete: []sdk.Incomplete{}, Imports: []string{}}
	for _, f := range found.Fragments {
		if own[f.File.Path] {
			res.Fragments = append(res.Fragments, f)
		}
	}
	for _, src := range found.Sources {
		if own[src.Path] {
			res.Sources = append(res.Sources, src)
		}
	}
	for _, gap := range found.Incomplete {
		if own[gap.Path] {
			res.Incomplete = append(res.Incomplete, gap)
		}
	}
	excluded, example := 0, ""
	for _, f := range u.Files {
		if r.excludedClaimed[f.Path] {
			excluded++
			if example == "" {
				example = f.Path
			}
		}
	}
	if excluded > 0 {
		res.Notices = append(res.Notices, fmt.Sprintf(
			"%d file(s) excluded by build constraints, e.g. %s; select another context with [languages.go] tags or env",
			excluded, example))
	}
	res.Body = digest(hashes...)
	res.Api = res.Body
	if d := s.asked[u.ID]; d != nil {
		res.Body, res.Api = d.content, apiOf(d)
		res.Imports = s.importsOf(d)
		res.Reads = s.pl.reads
	}
	return res
}

// regionsResult is the answer for the fences of the host files: it reads the
// project's packages as an importer does, so it imports them all.
func (r *run) regionsResult(s *shape, u sdk.UnitRef, snippets []*snippet) sdk.UnitResult {
	found := r.regionResult(answer{}, snippets)
	hashes := make([]string, 0, len(u.Regions))
	for _, host := range u.Regions {
		hashes = append(hashes, host.Path, host.Hash)
	}
	res := sdk.UnitResult{
		ID:         u.ID,
		Fragments:  found.Fragments,
		Notices:    found.Notices,
		Incomplete: found.Incomplete,
		Body:       digest(hashes...),
		Imports:    []string{},
	}
	res.Api = res.Body
	if res.Fragments == nil {
		res.Fragments = []sdk.Fragment{}
	}
	if res.Notices == nil {
		res.Notices = []string{}
	}
	if res.Incomplete == nil {
		res.Incomplete = []sdk.Incomplete{}
	}
	for _, d := range s.pl.dirs {
		if d != nil && !filepath.IsAbs(d.rel) {
			res.Imports = append(res.Imports, d.rel)
		}
	}
	sort.Strings(res.Imports)
	return res
}

func joinRel(dir, name string) string {
	if dir == "" || dir == "." {
		return name
	}
	return dir + "/" + name
}

// digest hashes parts without letting their boundaries blur.
func digest(parts ...string) string {
	h := sha256.New()
	for _, p := range parts {
		h.Write([]byte(strconv.Itoa(len(p))))
		h.Write([]byte{':'})
		h.Write([]byte(p))
	}
	return hex.EncodeToString(h.Sum(nil))
}

// contentHash is the SHA-256 the host sends for a file, in lowercase hex.
func contentHash(src []byte) string {
	sum := sha256.Sum256(src)
	return hex.EncodeToString(sum[:])
}

// factsOf reads the facts out of source text. A file that does not parse is
// treated as all API: any change to it changes every dependent.
func factsOf(src []byte) fileFacts {
	fset := token.NewFileSet()
	file, err := parser.ParseFile(fset, "", src, parser.SkipObjectResolution)
	if file == nil {
		whole := contentHash(src)
		return fileFacts{API: whole, Interfaces: whole, matching: matching{open: true, unknown: true}}
	}
	facts := fileFacts{Imports: importsOf(file), matching: matchingOf(file)}
	tf := fset.File(file.Pos())
	if err != nil || tf == nil {
		whole := contentHash(src)
		facts.API, facts.Interfaces = whole, whole
		return facts
	}
	offset := func(p token.Pos) int { return tf.Offset(p) }
	facts.API = apiDigest(src, file, offset)
	facts.Interfaces = interfaceDigest(src, file, offset)
	return facts
}

func importsOf(file *ast.File) []string {
	seen := map[string]bool{}
	var out []string
	for _, spec := range file.Imports {
		path, err := strconv.Unquote(spec.Path.Value)
		if err != nil || seen[path] {
			continue
		}
		seen[path] = true
		out = append(out, path)
	}
	sort.Strings(out)
	return out
}

// apiDigest hashes the text between the function bodies of a file.
func apiDigest(src []byte, file *ast.File, offset func(token.Pos) int) string {
	h := sha256.New()
	at := 0
	for _, decl := range file.Decls {
		fn, ok := decl.(*ast.FuncDecl)
		if !ok || fn.Body == nil {
			continue
		}
		start, end := offset(fn.Body.Lbrace), offset(fn.Body.Rbrace)+1
		if start < at || end > len(src) {
			return contentHash(src)
		}
		h.Write(src[at:start])
		at = end
	}
	h.Write(src[at:])
	return hex.EncodeToString(h.Sum(nil))
}

// interfaceDigest hashes the type specs whose type might be an interface:
// literal interfaces, and names or instantiations that could stand for one.
func interfaceDigest(src []byte, file *ast.File, offset func(token.Pos) int) string {
	h := sha256.New()
	found := false
	for _, decl := range file.Decls {
		gen, ok := decl.(*ast.GenDecl)
		if !ok || gen.Tok != token.TYPE {
			continue
		}
		for _, spec := range gen.Specs {
			ts, ok := spec.(*ast.TypeSpec)
			if !ok || !mayBeInterface(ts.Type) {
				continue
			}
			start, end := offset(ts.Pos()), offset(ts.End())
			if start < 0 || end > len(src) || start > end {
				return contentHash(src)
			}
			found = true
			h.Write(src[start:end])
			h.Write([]byte{0})
		}
	}
	if !found {
		return ""
	}
	return hex.EncodeToString(h.Sum(nil))
}

func mayBeInterface(expr ast.Expr) bool {
	switch t := expr.(type) {
	case *ast.Ident:
		if tn, ok := types.Universe.Lookup(t.Name).(*types.TypeName); ok {
			_, basic := tn.Type().Underlying().(*types.Basic)
			return !basic
		}
		return true
	case *ast.InterfaceType, *ast.SelectorExpr, *ast.IndexExpr, *ast.IndexListExpr, *ast.ParenExpr:
		return true
	default:
		return false
	}
}

// apiOf is what importers see of d: its own API and the API of everything it
// imports, so that it composes.
func apiOf(d *directory) string {
	parts := []string{d.rel, d.api}
	for _, dep := range closure(d) {
		parts = append(parts, dep.rel, dep.api)
	}
	return digest(parts...)
}

// closure is every unit u imports, directly or not.
func closure(u *directory) []*directory {
	seen := map[*directory]bool{u: true}
	var out []*directory
	for stack := slices.Clone(u.deps); len(stack) > 0; {
		d := stack[len(stack)-1]
		stack = stack[:len(stack)-1]
		if seen[d] {
			continue
		}
		seen[d] = true
		out = append(out, d)
		stack = append(stack, d.deps...)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].abs < out[j].abs })
	return out
}
