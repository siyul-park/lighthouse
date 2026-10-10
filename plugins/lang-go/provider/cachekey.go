package provider

import (
	"encoding/json"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"sort"
	"strconv"
	"strings"
	"sync"

	"golang.org/x/mod/modfile"
)

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
	imports   []string
	deps      []*cacheUnit
	// apiKey covers everything but the unit's own text; key covers that too.
	apiKey string
	key    string
}

// moduleRoot is a module that holds project code: its import path and the
// directory it lives in.
type moduleRoot struct{ path, dir string }

// planner computes the keys of the units of one request.
type planner struct {
	r       *run
	facts   map[string]fileFacts
	used    map[string]fileFacts
	modules []moduleRoot
	units   map[string]*cacheUnit
	// requestedUnits are the units that hold requested files, by directory.
	requestedUnits []*cacheUnit
}

// cacheOtherSuffixes are the non-Go files that go into a package.
var cacheOtherSuffixes = []string{".c", ".cc", ".cpp", ".cxx", ".h", ".hh", ".hpp", ".s", ".S", ".m", ".syso", ".f", ".swig", ".swigcxx"}

var executable struct {
	once sync.Once
	hash string
}

// executableHash identifies the provider binary: a rebuilt provider must not
// read what an older one wrote.
func executableHash() string {
	executable.once.Do(func() {
		if path, err := os.Executable(); err == nil {
			if data, err := os.ReadFile(path); err == nil {
				executable.hash = contentHash(data)
			}
		}
		if executable.hash == "" {
			executable.hash = "unknown"
		}
	})
	return executable.hash
}

// environment digests what the go command's answers depend on beyond the
// source: its own environment (version, GOOS/GOARCH, flags), the module files
// and the workspace. It is empty when the project cannot be cached safely: a
// vendor directory or a replace to a local path puts code outside the keys.
func (pl *planner) environment(batches []*batch) (string, []moduleRoot, bool) {
	env, err := pl.goEnv(batches[0].dir)
	if err != nil {
		return "", nil, false
	}
	parts := []string{env}
	var modules []moduleRoot
	for _, b := range batches {
		local := pl.r.localModule(b)
		if local == nil {
			return "", nil, false
		}
		modules = append(modules, moduleRoot{local.path, b.dir})
		if b.synthetic {
			continue
		}
		text, ok := moduleFiles(b.dir, "go.mod", "go.sum")
		if !ok {
			return "", nil, false
		}
		parts = append(parts, text, stamp(filepath.Join(b.dir, "vendor")))
	}
	work, extra, ok := workspaceOf(env)
	if !ok {
		return "", nil, false
	}
	return digest(append(parts, work)...), append(modules, extra...), true
}

// goEnvVars are the settings of the go command that decide which files make a
// package and what they mean: the toolchain, the target, the flags and the
// workspace. Others, such as GOGCCFLAGS, change from run to run.
var goEnvVars = []string{
	"GOVERSION", "GOROOT", "GOOS", "GOARCH", "GOFLAGS", "GOEXPERIMENT", "CGO_ENABLED", "GOWORK",
	"GOTOOLCHAIN", "GOAMD64", "GOARM", "GOARM64", "GO386", "GOMIPS", "GOMIPS64", "GOPPC64", "GORISCV64", "GOFIPS140",
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

// moduleFiles digests a module's go.mod and go.sum, and stamps the directories
// its replace directives point to.
func moduleFiles(dir, mod, sum string) (string, bool) {
	data, err := os.ReadFile(filepath.Join(dir, mod))
	if err != nil {
		return "", false
	}
	file, err := modfile.ParseLax(mod, data, nil)
	if err != nil {
		return "", false
	}
	sums, _ := os.ReadFile(filepath.Join(dir, sum))
	return digest(string(data), string(sums), localReplaces(dir, file.Replace)), true
}

// localReplaces stamps the directories that replace directives point to.
func localReplaces(dir string, replaces []*modfile.Replace) string {
	var stamps []string
	for _, rep := range replaces {
		if rep.New.Version != "" {
			continue
		}
		target := filepath.FromSlash(rep.New.Path)
		if !filepath.IsAbs(target) {
			target = filepath.Join(dir, target)
		}
		stamps = append(stamps, stamp(target))
	}
	return digest(stamps...)
}

// stamp fingerprints a directory tree by the names, sizes and modification
// times of its files, without reading them: code the project does not own is
// too large to hash on every run, and a touched file is only a miss.
func stamp(root string) string {
	var parts []string
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

// workspaceOf digests go.work, when the environment names one, and returns its
// modules.
func workspaceOf(env string) (string, []moduleRoot, bool) {
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
	for _, use := range file.Use {
		dir := filepath.Join(filepath.Dir(path), filepath.FromSlash(use.Path))
		mod, ok := os.ReadFile(filepath.Join(dir, "go.mod"))
		parsed, err := modfile.ParseLax("go.mod", mod, nil)
		if ok != nil || err != nil || parsed.Module == nil {
			return "", nil, false
		}
		modules = append(modules, moduleRoot{parsed.Module.Mod.Path, dir})
	}
	sums, _ := os.ReadFile(path + ".sum")
	return digest(string(data), string(sums), localReplaces(filepath.Dir(path), file.Replace)), modules, true
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
	var content, api, ifaces, imports []string // ifaces holds only the non-empty
	for _, e := range entries {
		name := e.Name()
		switch {
		case e.IsDir():
		case strings.HasSuffix(name, ".go"):
			relFile := joinRel(rel, name)
			_, requested := pl.r.requested[relFile]
			facts, hash, ok := pl.factsOfFile(abs, relFile)
			if !ok {
				return nil
			}
			if requested {
				u.requested = append(u.requested, relFile)
			}
			content = append(content, name, hash, boolText(requested))
			api = append(api, name, facts.API)
			if facts.Interfaces != "" {
				ifaces = append(ifaces, name, facts.Interfaces)
			}
			imports = append(imports, facts.Imports...)
		case slices.ContainsFunc(cacheOtherSuffixes, func(s string) bool { return strings.HasSuffix(name, s) }):
			info, err := e.Info()
			if err != nil {
				return nil
			}
			content = append(content, name, digest(info.ModTime().String(), strconv.FormatInt(info.Size(), 10)))
			api = append(api, name, "other")
		}
	}
	if len(content) == 0 {
		return nil
	}
	sort.Strings(u.requested)
	slices.Sort(imports)
	u.imports = slices.Compact(imports)
	u.content = digest(content...)
	u.api = digest(api...)
	if len(ifaces) > 0 {
		u.ifaces = digest(append([]string{u.rel}, ifaces...)...)
	}
	return u
}

// factsOfFile returns the facts and content hash of a file, parsing it only
// when the cache has not seen that content.
func (pl *planner) factsOfFile(abs, rel string) (fileFacts, string, bool) {
	path := filepath.Join(abs, filepath.Base(rel))
	if f, ok := pl.r.requested[rel]; ok && isContentHash(f.Hash) {
		if facts, known := pl.facts[f.Hash]; known {
			pl.used[f.Hash] = facts
			return facts, f.Hash, true
		}
	}
	src, err := os.ReadFile(path)
	if err != nil {
		return fileFacts{}, "", false
	}
	hash := contentHash(src)
	facts, known := pl.facts[hash]
	if !known {
		facts = factsOf(src)
	}
	pl.used[hash] = facts
	return facts, hash, true
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
