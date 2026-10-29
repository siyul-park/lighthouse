package provider

import (
	"encoding/json"
	"go/parser"
	"go/token"
	"io/fs"
	"os"
	"os/exec"
	"path"
	"path/filepath"
	"strings"
	"sync"

	"golang.org/x/mod/modfile"
	"golang.org/x/mod/module"
)

// known are the packages a name can be settled to without asking goimports,
// which scans the module and its dependencies for every call: the standard
// library and the packages of the module itself, by the name a file uses them
// under.
type known map[string][]importSpec

// packageSpec is a package of a directory tree: the import path and the name.
type packageSpec struct {
	name string
	path string
}

// toolchain is where the run's go command keeps the standard library and the
// modules it downloaded.
type toolchain struct {
	GOROOT     string
	GOMODCACHE string
}

var standardPackages sync.Map // GOROOT -> []packageSpec

func (k known) add(spec packageSpec) {
	k[spec.name] = append(k[spec.name], importSpec{path: spec.path})
}

// knownPackages are the packages of the standard library and of the module in
// dir. The standard library is read once per toolchain.
func (r *run) knownPackages(dir string) known {
	out := known{}
	for _, spec := range r.standard() {
		out.add(spec)
	}
	for _, spec := range modulePackages(dir) {
		out.add(spec)
	}
	return out
}

// standard are the packages of the standard library the run's toolchain has.
func (r *run) standard() []packageSpec {
	goroot := r.toolchain().GOROOT
	if goroot == "" {
		return nil
	}
	if known, ok := standardPackages.Load(goroot); ok {
		return known.([]packageSpec)
	}
	specs := packagesUnder(filepath.Join(goroot, "src"), "", func(rel string) bool {
		return rel == "cmd" || rel == "vendor"
	})
	standardPackages.Store(goroot, specs)
	return specs
}

// toolchain reads them once per run; empty when the go command does not say.
func (r *run) toolchain() toolchain {
	if r.chain != nil {
		return *r.chain
	}
	var chain toolchain
	cmd := exec.Command(r.opts.bin, "env", "-json", "GOROOT", "GOMODCACHE")
	cmd.Env = r.opts.environ()
	if out, err := cmd.Output(); err == nil {
		_ = json.Unmarshal(out, &chain)
	}
	r.chain = &chain
	return chain
}

// required settles the names that a module the one in dir requires has for
// its root package, from the module cache. The names a requirement may have
// further down are the ones it returns as plausible: only goimports knows.
func (r *run) required(dir string, names map[string]bool) (settled map[string]importSpec, plausible map[string]bool) {
	settled, plausible = map[string]importSpec{}, map[string]bool{}
	data, err := os.ReadFile(filepath.Join(dir, "go.mod"))
	if err != nil {
		return settled, plausible
	}
	file, err := modfile.ParseLax("go.mod", data, nil)
	if err != nil {
		return settled, plausible
	}
	cache := r.toolchain().GOMODCACHE
	for _, req := range file.Require {
		name := importName(req.Mod.Path)
		if !names[name] {
			continue
		}
		if _, again := settled[name]; again || plausible[name] {
			delete(settled, name)
			plausible[name] = true
			continue
		}
		escapedPath, pathErr := module.EscapePath(req.Mod.Path)
		escapedVersion, versionErr := module.EscapeVersion(req.Mod.Version)
		if pathErr != nil || versionErr != nil || cache == "" || packageClause(filepath.Join(cache, escapedPath+"@"+escapedVersion)) != name {
			plausible[name] = true
			continue
		}
		settled[name] = importSpec{path: req.Mod.Path}
	}
	return settled, plausible
}

// modulePackages are the packages of the module whose go.mod is in dir, under
// the module path.
func modulePackages(dir string) []packageSpec {
	data, err := os.ReadFile(filepath.Join(dir, "go.mod"))
	if err != nil {
		return nil
	}
	file, err := modfile.ParseLax("go.mod", data, nil)
	if err != nil || file.Module == nil {
		return nil
	}
	return packagesUnder(dir, file.Module.Mod.Path, func(string) bool { return false })
}

// packagesUnder lists the importable packages under root, whose import path
// is prefix followed by the directory: the ones the go tool does not skip, and
// not those of another module, `internal` ones, or `main`.
func packagesUnder(root, prefix string, skip func(rel string) bool) []packageSpec {
	var specs []packageSpec
	_ = filepath.WalkDir(root, func(at string, entry fs.DirEntry, err error) error {
		if err != nil || !entry.IsDir() {
			return nil
		}
		name := entry.Name()
		rel, relErr := filepath.Rel(root, at)
		if relErr != nil {
			return filepath.SkipDir
		}
		rel = filepath.ToSlash(rel)
		if rel != "." && (name == "testdata" || name == "vendor" || name == "internal" || strings.HasPrefix(name, ".") || strings.HasPrefix(name, "_") || skip(rel)) {
			return filepath.SkipDir
		}
		if rel != "." {
			if _, err := os.Stat(filepath.Join(at, "go.mod")); err == nil {
				return filepath.SkipDir
			}
		}
		if pkg := packageClause(at); pkg != "" && pkg != "main" {
			specs = append(specs, packageSpec{name: pkg, path: path.Join(prefix, rel)})
		}
		return nil
	})
	return specs
}

// packageClause is the package name the Go files of dir declare, "" when it
// holds none.
func packageClause(dir string) string {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return ""
	}
	for _, entry := range entries {
		name := entry.Name()
		if entry.IsDir() || !strings.HasSuffix(name, ".go") || strings.HasSuffix(name, "_test.go") {
			continue
		}
		file, err := parser.ParseFile(token.NewFileSet(), filepath.Join(dir, name), nil, parser.PackageClauseOnly)
		if err == nil {
			return file.Name.Name
		}
	}
	return ""
}
