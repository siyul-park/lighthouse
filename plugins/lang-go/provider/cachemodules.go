package provider

import (
	"encoding/json"
	"io/fs"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"golang.org/x/mod/modfile"
)

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

// identity names the running binary by its size and modification time: a
// rebuilt provider must not read what an older one wrote.
func (p *Provider) identity() string {
	if p.build != "" {
		return p.build
	}
	p.build = "unknown"
	if path, err := os.Executable(); err == nil {
		if info, err := os.Stat(path); err == nil {
			p.build = digest(path, strconv.FormatInt(info.Size(), 10), strconv.FormatInt(info.ModTime().UnixNano(), 10))
		}
	}
	return p.build
}

// within reports whether path is root or lies under it, links resolved.
func within(root, path string) bool {
	if resolved, err := filepath.EvalSymlinks(root); err == nil {
		root = resolved
	}
	rel, err := filepath.Rel(root, path)
	return err == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator))
}
