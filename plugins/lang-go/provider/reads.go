package provider

import (
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"

	"golang.org/x/mod/modfile"
)

// outside adds the modules the project builds with that live outside it (the
// targets of local `replace` directives and the members of a go.work), so
// their packages are linked as the imports of the units that use them, and
// lists the files outside the project the answers depend on: the host records
// them with each answer and asks again when one changed.
func (pl *planner) outside(batches []*batch) {
	seen := map[string]bool{}
	add := func(path string) {
		if !seen[path] {
			seen[path] = true
			pl.reads = append(pl.reads, path)
		}
	}
	module := func(dir, path string) {
		if mod, ok := moduleAt(dir); ok {
			pl.modules = append(pl.modules, moduleRoot{path: pathOr(path, mod), dir: dir})
		}
		walkSources(dir, add)
	}
	replaces := func(base string, list []*modfile.Replace) {
		for _, rep := range list {
			if rep.New.Version != "" {
				continue
			}
			if dir, ok := pl.elsewhere(base, rep.New.Path); ok {
				module(dir, rep.Old.Path)
			}
		}
	}
	for _, b := range batches {
		if b.synthetic {
			continue
		}
		if data, err := os.ReadFile(filepath.Join(b.dir, "go.mod")); err == nil {
			if file, err := modfile.Parse("go.mod", data, nil); err == nil {
				replaces(b.dir, file.Replace)
			}
		}
		if modules := filepath.Join(b.dir, "vendor", "modules.txt"); fileExists(modules) {
			add(modules)
		}
	}
	if len(batches) > 0 {
		pl.workspace(batches[0].dir, add, module, replaces)
	}
	sort.Strings(pl.reads)
}

// workspace reads the go.work the go command uses in dir, if there is one.
func (pl *planner) workspace(dir string, add func(string), module func(dir, path string), replaces func(string, []*modfile.Replace)) {
	cmd := exec.Command(pl.r.opts.bin, "env", "GOWORK")
	cmd.Dir = dir
	cmd.Env = pl.r.opts.environ()
	out, err := cmd.Output()
	work := strings.TrimSpace(string(out))
	if err != nil || work == "" || work == "off" {
		return
	}
	data, err := os.ReadFile(work)
	if err != nil {
		return
	}
	file, err := modfile.ParseWork(work, data, nil)
	if err != nil {
		return
	}
	base := filepath.Dir(work)
	if _, inside := pl.r.rel(work); !inside {
		add(work)
		add(work + ".sum")
	}
	for _, use := range file.Use {
		if target, ok := pl.elsewhere(base, use.Path); ok {
			module(target, "")
		}
	}
	replaces(base, file.Replace)
}

// elsewhere is the directory a path of a go.mod or go.work in base names,
// when it is outside the project.
func (pl *planner) elsewhere(base, path string) (string, bool) {
	target := filepath.FromSlash(path)
	if !filepath.IsAbs(target) {
		target = filepath.Join(base, target)
	}
	if resolved, err := filepath.EvalSymlinks(target); err == nil {
		target = resolved
	}
	if _, inside := pl.r.rel(target); inside {
		return "", false
	}
	info, err := os.Stat(target)
	return target, err == nil && info.IsDir()
}

// moduleAt is the module path the go.mod in dir declares.
func moduleAt(dir string) (string, bool) {
	data, err := os.ReadFile(filepath.Join(dir, "go.mod"))
	if err != nil {
		return "", false
	}
	file, err := modfile.ParseLax("go.mod", data, nil)
	if err != nil || file.Module == nil {
		return "", false
	}
	return file.Module.Mod.Path, true
}

// pathOr is the import path a `replace` gives the module, else the one the
// module declares.
func pathOr(replaced, declared string) string {
	if replaced != "" {
		return replaced
	}
	return declared
}

// walkSources calls add with the Go sources and module files under dir.
func walkSources(dir string, add func(string)) {
	_ = filepath.WalkDir(dir, func(path string, entry fs.DirEntry, err error) error {
		if err != nil || entry.IsDir() {
			return nil
		}
		if name := entry.Name(); strings.HasSuffix(name, ".go") || name == "go.mod" || name == "go.sum" {
			add(path)
		}
		return nil
	})
}

func fileExists(path string) bool {
	info, err := os.Stat(path)
	return err == nil && !info.IsDir()
}
