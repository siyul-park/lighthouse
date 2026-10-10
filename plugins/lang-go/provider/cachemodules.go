package provider

import (
	"encoding/json"
	"io/fs"
	"os"
	"path/filepath"
	"strconv"
	"sync"

	"golang.org/x/mod/modfile"
)

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
