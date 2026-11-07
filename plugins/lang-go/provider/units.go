package provider

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"go/ast"
	"go/parser"
	"go/scanner"
	"go/token"
	"hash"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"strconv"
	"strings"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

// shape is how a request is analyzed: which directories are asked for and the
// `api` of each.
type shape struct {
	pl *planner
	// asked are the directories of the units of files, by the host's id.
	asked map[string]*directory
}

// planner reads the directories asked for and tells what the host needs to
// know of each: its `api` and the directories it imports.
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
	imports   []string
	deps      []*directory
}

// moduleRoot is a module that holds project code: its import path and the
// directory it lives in.
type moduleRoot struct{ path, dir string }

// dirDigest collects what the files of one directory contribute to its unit.
type dirDigest struct {
	content, api, imports []string
}

// fileFacts are what the host needs to know about one Go source file, derived
// from its text alone.
type fileFacts struct {
	// Imports are the import paths of the file.
	Imports []string
	// API digests each declaration of the file without the bodies of its
	// functions and methods. An edit inside a body leaves it unchanged, and with
	// it the `api` of every package that depends on this one (early cutoff).
	API []string
	// Directives digests the directives outside the declarations; empty when
	// there are none. They belong to the file, not to a declaration.
	Directives string
}

// declSpan is where a top-level declaration lies in the source: from its doc
// comment to its end, and the braces of its body, -1 when it has none.
type declSpan struct{ start, end, bodyStart, bodyEnd int }

// everyUnit is the import that stands for every unit of the language.
const everyUnit = "*"

// goEnvVars are the settings of the go command that decide which files make a
// package and what they mean: the toolchain, the target, the flags and the
// workspace. The host puts their values in the key of every unit.
var goEnvVars = []string{
	"GOROOT", "GOOS", "GOARCH", "GOFLAGS", "GOEXPERIMENT", "CGO_ENABLED", "GOWORK",
	"GOTOOLCHAIN", "GOAMD64", "GOARM", "GOARM64", "GO386", "GOMIPS", "GOMIPS64", "GOPPC64", "GORISCV64", "GOFIPS140",
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
	if !strings.HasSuffix(name, "_test.go") {
		d.api = append(d.api, facts.API...)
		if facts.Directives != "" {
			d.api = append(d.api, digest(name, facts.Directives))
		}
	}
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
	slices.Sort(d.api)
	u.api = digest(d.api...)
}

// plan reads the directories asked for, and those they import, to tell the
// host what each imports and what an importer sees of it.
func (r *run) plan(params sdk.IndexParams) *shape {
	pl := &planner{r: r, dirs: map[string]*directory{}}
	s := &shape{pl: pl, asked: map[string]*directory{}}
	batches := r.batches(params.Files)
	if !pl.moduleRoots(batches) {
		return s
	}
	pl.outside(batches)
	for _, u := range params.Units {
		if len(u.Files) > 0 {
			s.asked[u.ID] = pl.unit(filepath.Join(r.root, filepath.FromSlash(u.ID)))
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
			out = append(out, r.regionsResult(u, snippets))
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
		res.Imports = importedUnits(d)
		res.Reads = s.pl.reads
	}
	return res
}

// regionsResult is the answer for the fences of the host files: it reads the
// project's packages as an importer does, so it imports every unit.
func (r *run) regionsResult(u sdk.UnitRef, snippets []*snippet) sdk.UnitResult {
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
		Imports:    []string{everyUnit},
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
	return res
}

// importedUnits are the directories d imports, directly or not, sorted: the ones
// whose `api` the `api` of d covers, so that a unit is asked again in the round
// after one of them moved and not one round per level of imports.
func importedUnits(d *directory) []string {
	out := []string{}
	for _, dep := range closure(d) {
		if !filepath.IsAbs(dep.rel) {
			out = append(out, dep.rel)
		}
	}
	sort.Strings(out)
	return out
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
		return fileFacts{API: []string{whole}}
	}
	facts := fileFacts{Imports: importsOf(file)}
	tf := fset.File(file.Pos())
	if err != nil || tf == nil {
		whole := contentHash(src)
		facts.API = []string{whole}
		return facts
	}
	offset := func(p token.Pos) int { return tf.Offset(p) }
	facts.API, facts.Directives = apiDigest(src, file, offset)
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

// apiDigest is what an importer can see of a file, one digest per top-level
// declaration, and the directives that stand outside them, with no file name in
// either: where a declaration is written is not part of what it declares. A
// declaration is its tokens without the bodies of its functions and methods.
// Comments and layout are not in it, except the directives (`//go:build`,
// `//go:embed`, `// +build`), which change what the file declares; a doc comment
// is a fact of the fragment. The directives outside the declarations are one
// text, which belongs to the file.
func apiDigest(src []byte, file *ast.File, offset func(token.Pos) int) (decls []string, directives string) {
	spans, ok := declSpans(src, file, offset)
	if !ok {
		return []string{contentHash(src)}, ""
	}
	fset := token.NewFileSet()
	tf := fset.AddFile("", fset.Base(), len(src))
	var scan scanner.Scanner
	scan.Init(tf, src, nil, scanner.ScanComments)
	hashes := make([]hash.Hash, len(spans))
	for i := range hashes {
		hashes[i] = sha256.New()
	}
	header, outside := sha256.New(), new(strings.Builder)
	packageEnd := offset(file.Name.End())
	next := 0
	for {
		pos, tok, lit := scan.Scan()
		if tok == token.EOF {
			break
		}
		at := tf.Offset(pos)
		for next < len(spans) && at >= spans[next].end {
			next++
		}
		isDirective := tok == token.COMMENT && directive(lit)
		if tok == token.COMMENT && !isDirective {
			continue
		}
		if lit == "" {
			lit = tok.String()
		}
		switch {
		case next < len(spans) && at >= spans[next].start:
			if !inBody(spans[next], at) {
				fmt.Fprintf(hashes[next], "%d:%s\x00", tok, lit)
			}
		case isDirective:
			fmt.Fprintf(outside, "%d:%s\x00", tok, lit)
		case at < packageEnd:
			fmt.Fprintf(header, "%d:%s\x00", tok, lit)
		}
	}
	decls = append(decls, hex.EncodeToString(header.Sum(nil)))
	for _, h := range hashes {
		decls = append(decls, hex.EncodeToString(h.Sum(nil)))
	}
	if outside.Len() > 0 {
		directives = contentHash([]byte(outside.String()))
	}
	return decls, directives
}

// inBody reports whether the offset lies in the body of the function of sp.
func inBody(sp declSpan, at int) bool {
	return sp.bodyStart >= 0 && at >= sp.bodyStart && at < sp.bodyEnd
}

// declSpans are the spans of the declarations of file; false when a position
// lies outside src.
func declSpans(src []byte, file *ast.File, offset func(token.Pos) int) ([]declSpan, bool) {
	spans := make([]declSpan, 0, len(file.Decls))
	for _, decl := range file.Decls {
		sp := declSpan{start: offset(decl.Pos()), end: offset(decl.End()), bodyStart: -1, bodyEnd: -1}
		switch d := decl.(type) {
		case *ast.GenDecl:
			if d.Doc != nil {
				sp.start = offset(d.Doc.Pos())
			}
		case *ast.FuncDecl:
			if d.Doc != nil {
				sp.start = offset(d.Doc.Pos())
			}
			if d.Body != nil {
				sp.bodyStart, sp.bodyEnd = offset(d.Body.Lbrace), offset(d.Body.Rbrace)+1
			}
		}
		if sp.start < 0 || sp.end > len(src) || sp.start > sp.end || sp.bodyStart > sp.bodyEnd || sp.bodyEnd > sp.end {
			return nil, false
		}
		spans = append(spans, sp)
	}
	return spans, true
}

// directive reports whether a comment is a build or compiler directive.
func directive(comment string) bool {
	for _, prefix := range []string{"//go:", "//+build", "// +build", "//line "} {
		if strings.HasPrefix(comment, prefix) {
			return true
		}
	}
	return false
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
