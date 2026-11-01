package provider

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"go/ast"
	"go/parser"
	"go/token"
	"go/types"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"sort"
	"strconv"
	"strings"

	"golang.org/x/mod/modfile"
)

// planner computes the keys of the units of one request.
type planner struct {
	r *run
	// edits are the texts that stand in for files, by absolute path.
	edits   map[string][]byte
	facts   map[string]fileFacts
	used    map[string]fileFacts
	modules []moduleRoot
	units   map[string]*cacheUnit
	// requestedUnits are the units that hold requested files, by directory.
	requestedUnits []*cacheUnit
}

// cacheUnit is a package directory, the unit of the cache. Its key covers
// everything its fragments are computed from: its own files (all of them, not
// only the requested ones), the API of the directories it imports, directly or
// not, and the interface declarations of the whole project.
type cacheUnit struct {
	rel, abs  string
	requested []string // project-relative files the host asked for, sorted
	content   string
	api       string
	ifaces    string
	cgo       bool // imports "C": always analyzed, its record only serves implements reuse
	imports   []string
	deps      []*cacheUnit
	// apiKey covers everything but the unit's own text; key covers that too.
	apiKey string
	key    string
}

// moduleRoot is a module that holds project code: its import path and the
// directory it lives in.
type moduleRoot struct{ path, dir string }

// dirDigest collects what the files of one directory contribute to its unit.
type dirDigest struct {
	content, api, ifaces, imports []string // ifaces holds only the non-empty
	cgo                           bool
}

// fileFacts are what the unit cache needs to know about one Go source file,
// derived from its text alone.
type fileFacts struct {
	// Imports are the import paths of the file.
	Imports []string `json:"imports"`
	// API digests the text of the file without the bodies of its functions and
	// methods. An edit inside a body leaves it unchanged, and with it the key of
	// every package that depends on this one (early cutoff).
	API string `json:"api"`
	// Interfaces digests the type declarations that may declare an interface;
	// empty when there are none. `implements` edges read every project
	// interface, so any change here changes every key.
	Interfaces string `json:"interfaces"`
	// Cgo is set when the file imports "C": its package is never cached, as the
	// C side is outside the keys.
	Cgo bool `json:"cgo"`
}

// cacheOtherSuffixes are the non-Go files that go into a package.
var cacheOtherSuffixes = []string{".c", ".cc", ".cpp", ".cxx", ".h", ".hh", ".hpp", ".s", ".S", ".m", ".syso", ".f", ".swig", ".swigcxx"}

// goEnvVars are the settings of the go command that decide which files make a
// package and what they mean: the toolchain, the target, the flags and the
// workspace. Others, such as GOGCCFLAGS, change from run to run.
var goEnvVars = []string{
	"GOVERSION", "GOROOT", "GOOS", "GOARCH", "GOFLAGS", "GOEXPERIMENT", "CGO_ENABLED", "GOWORK",
	"GOTOOLCHAIN", "GOAMD64", "GOARM", "GOARM64", "GO386", "GOMIPS", "GOMIPS64", "GOPPC64", "GORISCV64", "GOFIPS140",
}

// environment digests what the go command's answers depend on beyond the
// source: its own environment (version, GOOS/GOARCH, flags), the module files
// and the workspace. It is empty when the project cannot be cached safely.
func (pl *planner) environment(batches []*batch) (string, []moduleRoot, bool) {
	var parts []string
	var modules []moduleRoot
	for _, b := range batches {
		env, err := pl.goEnv(b.dir)
		if err != nil {
			return "", nil, false
		}
		work, extra, ok := workspaceOf(pl.r.root, env)
		if !ok {
			return "", nil, false
		}
		parts = append(parts, env, work)
		modules = append(modules, extra...)
		local := pl.r.localModule(b)
		if local == nil {
			return "", nil, false
		}
		modules = append(modules, moduleRoot{local.path, b.dir})
		if b.synthetic {
			continue
		}
		text, ok := moduleFiles(pl.r.root, b.dir, "go.mod", "go.sum")
		if !ok {
			return "", nil, false
		}
		parts = append(parts, text, vendorStamp(filepath.Join(b.dir, "vendor")))
	}
	return digest(parts...), modules, true
}

// interfaceDigest digests the interface declarations of every unit that is
// analyzed: a type's `implements` edges depend on all of them.
func (pl *planner) interfaceDigest() string {
	var parts []string
	for _, u := range pl.requestedUnits {
		parts = append(parts, u.rel, u.ifaces)
	}
	return digest(parts...)
}

// goEnv is the output of `go env -json` for goEnvVars.
func (pl *planner) goEnv(dir string) (string, error) {
	cmd := exec.Command(pl.r.opts.bin, append([]string{"env", "-json"}, goEnvVars...)...)
	cmd.Dir = dir
	cmd.Env = pl.r.opts.environ()
	out, err := cmd.Output()
	if err != nil {
		return "", err
	}
	return string(out), nil
}

// unit returns the unit of an absolute directory, reading it once. A directory
// without Go files is no unit.
func (pl *planner) unit(abs string) *cacheUnit {
	if u, known := pl.units[abs]; known {
		return u
	}
	u := pl.read(abs)
	pl.units[abs] = u
	if u != nil {
		pl.link(u)
	}
	return u
}

func (pl *planner) read(abs string) *cacheUnit {
	entries, err := os.ReadDir(abs)
	if err != nil {
		return nil
	}
	rel, inside := pl.r.rel(abs)
	if !inside {
		rel = abs
	}
	u := &cacheUnit{rel: rel, abs: abs}
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
func (pl *planner) entry(u *cacheUnit, d *dirDigest, e os.DirEntry) bool {
	name := e.Name()
	switch {
	case e.IsDir():
		return true
	case strings.HasSuffix(name, ".go"):
		return pl.goFile(u, d, name)
	case slices.ContainsFunc(cacheOtherSuffixes, func(s string) bool { return strings.HasSuffix(name, s) }):
		info, err := e.Info()
		if err != nil {
			return false
		}
		d.content = append(d.content, name, digest(info.ModTime().String(), strconv.FormatInt(info.Size(), 10)))
		d.api = append(d.api, name, "other")
	}
	return true
}

func (pl *planner) goFile(u *cacheUnit, d *dirDigest, name string) bool {
	rel := joinRel(u.rel, name)
	_, requested := pl.r.requested[rel]
	facts, hash, ok := pl.factsOfFile(u.abs, rel)
	if !ok {
		return false
	}
	if requested {
		u.requested = append(u.requested, rel)
	}
	d.content = append(d.content, name, hash, boolText(requested))
	d.api = append(d.api, name, facts.API)
	if facts.Interfaces != "" {
		d.ifaces = append(d.ifaces, name, facts.Interfaces)
	}
	d.imports = append(d.imports, facts.Imports...)
	d.cgo = d.cgo || facts.Cgo
	return true
}

// factsOfFile returns the facts and content hash of a file, parsing it only
// when the cache has not seen that content.
func (pl *planner) factsOfFile(abs, rel string) (fileFacts, string, bool) {
	path := filepath.Join(abs, filepath.Base(rel))
	src, edited := pl.edits[path]
	if f, ok := pl.r.requested[rel]; ok && isContentHash(f.Hash) && !edited {
		if facts, known := pl.facts[f.Hash]; known {
			pl.used[f.Hash] = facts
			return facts, f.Hash, true
		}
	}
	if !edited {
		var err error
		if src, err = os.ReadFile(path); err != nil {
			return fileFacts{}, "", false
		}
	}
	hash := contentHash(src)
	facts, known := pl.facts[hash]
	if !known {
		facts = factsOf(src)
	}
	pl.used[hash] = facts
	return facts, hash, true
}

// link resolves the imports of u into units.
func (pl *planner) link(u *cacheUnit) {
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
func (d *dirDigest) seal(u *cacheUnit) {
	u.cgo = d.cgo
	sort.Strings(u.requested)
	slices.Sort(d.imports)
	u.imports = slices.Compact(d.imports)
	u.content = digest(d.content...)
	u.api = digest(d.api...)
	if len(d.ifaces) > 0 {
		u.ifaces = digest(append([]string{u.rel}, d.ifaces...)...)
	}
}

// closure is every unit u imports, directly or not.
func closure(u *cacheUnit) []*cacheUnit {
	seen := map[*cacheUnit]bool{u: true}
	var out []*cacheUnit
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

func joinRel(dir, name string) string {
	if dir == "" || dir == "." {
		return name
	}
	return dir + "/" + name
}

func boolText(b bool) string {
	if b {
		return "1"
	}
	return "0"
}

// isContentHash reports whether the host sent a real SHA-256 in lowercase hex;
// anything else is read from the file.
func isContentHash(h string) bool {
	if len(h) != 64 {
		return false
	}
	for _, c := range h {
		if (c < '0' || c > '9') && (c < 'a' || c > 'f') {
			return false
		}
	}
	return true
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
		return fileFacts{API: whole, Interfaces: whole}
	}
	facts := fileFacts{Imports: importsOf(file)}
	facts.Cgo = slices.Contains(facts.Imports, "C")
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

// moduleFiles digests a module's go.mod and go.sum, and stamps the directories
// its replace directives point to.
func moduleFiles(root, dir, mod, sum string) (string, bool) {
	data, err := os.ReadFile(filepath.Join(dir, mod))
	if err != nil {
		return "", false
	}
	file, err := modfile.Parse(mod, data, nil)
	if err != nil {
		return "", false
	}
	sums, _ := os.ReadFile(filepath.Join(dir, sum))
	return digest(string(data), string(sums), localReplaces(root, dir, file.Replace)), true
}

// workspaceOf digests go.work, when the environment names one, and returns its
// modules.
func workspaceOf(root, env string) (string, []moduleRoot, bool) {
	var vars map[string]string
	if json.Unmarshal([]byte(env), &vars) != nil {
		return "", nil, false
	}
	path := vars["GOWORK"]
	if path == "" || path == "off" {
		return "", nil, true
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return "", nil, false
	}
	file, err := modfile.ParseWork(path, data, nil)
	if err != nil {
		return "", nil, false
	}
	var modules []moduleRoot
	var members []string
	for _, use := range file.Use {
		dir := filepath.Join(filepath.Dir(path), filepath.FromSlash(use.Path))
		mod, ok := os.ReadFile(filepath.Join(dir, "go.mod"))
		parsed, err := modfile.ParseLax("go.mod", mod, nil)
		if ok != nil || err != nil || parsed.Module == nil {
			return "", nil, false
		}
		modules = append(modules, moduleRoot{parsed.Module.Mod.Path, dir})
		member, found := moduleFiles(root, dir, "go.mod", "go.sum")
		if !found {
			return "", nil, false
		}
		members = append(members, member)
	}
	sums, _ := os.ReadFile(path + ".sum")
	return digest(append(members, string(data), string(sums), localReplaces(root, filepath.Dir(path), file.Replace))...), modules, true
}

// localReplaces digests the code outside the project that replace directives
// point to, by content: the keys must see it.
func localReplaces(root, dir string, replaces []*modfile.Replace) string {
	var parts []string
	for _, rep := range replaces {
		if rep.New.Version != "" {
			continue
		}
		target := filepath.FromSlash(rep.New.Path)
		if !filepath.IsAbs(target) {
			target = filepath.Join(dir, target)
		}
		if resolved, err := filepath.EvalSymlinks(target); err == nil {
			target = resolved
		}
		if within(root, target) {
			continue // project code: the keys of its units cover it
		}
		parts = append(parts, rep.New.Path, contents(target))
	}
	return digest(parts...)
}

// contents hashes the Go sources and module files under root.
func contents(root string) string {
	var parts []string
	_ = filepath.WalkDir(root, func(path string, d fs.DirEntry, err error) error {
		if err != nil || d.IsDir() {
			return nil
		}
		if name := d.Name(); !(strings.HasSuffix(name, ".go") || name == "go.mod") {
			return nil
		}
		if data, err := os.ReadFile(path); err == nil {
			parts = append(parts, path, contentHash(data))
		}
		return nil
	})
	return digest(parts...)
}

// vendorStamp fingerprints a vendor directory by the names, sizes and
// modification times of its files, and by the content of modules.txt: code the
// project does not own is too large to hash on every run.
func vendorStamp(root string) string {
	modules, _ := os.ReadFile(filepath.Join(root, "modules.txt"))
	parts := []string{contentHash(modules)}
	_ = filepath.WalkDir(root, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if info, err := d.Info(); err == nil && !d.IsDir() {
			parts = append(parts, path, strconv.FormatInt(info.Size(), 10), strconv.FormatInt(info.ModTime().UnixNano(), 10))
		}
		return nil
	})
	return digest(parts...)
}

// within reports whether path is root or lies under it, links resolved.
func within(root, path string) bool {
	if resolved, err := filepath.EvalSymlinks(root); err == nil {
		root = resolved
	}
	rel, err := filepath.Rel(root, path)
	return err == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator))
}
