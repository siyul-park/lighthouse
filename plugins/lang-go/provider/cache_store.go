package provider

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"strings"
	"time"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

// cacheStore is the directory the host lets the provider keep results in.
// Every read problem is a miss; the first write problem is remembered and
// stops further writes.
type cacheStore struct {
	dir    string
	failed error
	// listing is the directory as it was when first needed.
	listing []os.DirEntry
	listed  bool
}

// unitRecord is what the cache keeps of a unit: the fragments of its requested
// files, as JSON, and which of the files the build constraints excluded.
type unitRecord struct {
	// API is the key of the unit without its own text. A body edit leaves it
	// unchanged, and then the `implements` edges of the old record still hold.
	API      string       `json:"api"`
	Files    []recordFile `json:"files"`
	Excluded []string     `json:"excluded"`
}

// recordFile is the encoded fragment of one file. It is never decoded on a
// hit: the answer carries the bytes as they are.
type recordFile struct {
	Path     string          `json:"path"`
	Fragment json.RawMessage `json:"fragment"`
	// Far are the methods of the file that implement only an interface of a
	// package of the project that the file's package does not import: the one
	// thing in a fragment that an analysis of the package alone cannot see.
	Far []string `json:"far,omitempty"`
}

const (
	// cacheSchema versions the layout and meaning of everything in the cache
	// directory; changing it makes every entry a miss.
	cacheSchema = "lang-go-cache/4"
	// cacheLimit caps the size of the cache directory; the oldest entries go first.
	cacheLimit = 256 << 20

	unitPrefix = "u-"
	factsFile  = "facts.json"
)

func (s *cacheStore) read(name string, into any) bool {
	data, err := os.ReadFile(filepath.Join(s.dir, name))
	if err != nil {
		return false
	}
	return json.Unmarshal(data, into) == nil
}

// write stores value under name, atomically: a reader sees the old file or the
// new one, never half of it.
func (s *cacheStore) write(name string, value any) {
	if s.failed != nil {
		return
	}
	data, err := json.Marshal(value)
	if err == nil {
		err = os.MkdirAll(s.dir, 0o755)
	}
	if err == nil {
		err = s.replace(name, data)
	}
	s.failed = err
}

func (s *cacheStore) replace(name string, data []byte) error {
	tmp, err := os.CreateTemp(s.dir, ".tmp-*")
	if err != nil {
		return err
	}
	_, err = tmp.Write(data)
	if closeErr := tmp.Close(); err == nil {
		err = closeErr
	}
	if err == nil {
		err = os.Rename(tmp.Name(), filepath.Join(s.dir, name))
	}
	if err != nil {
		_ = os.Remove(tmp.Name())
	}
	return err
}

// notice explains a cache that could not be written, for the run's notices.
func (s *cacheStore) notice() string {
	if s.failed == nil {
		return ""
	}
	return fmt.Sprintf("cache directory %s is not writable, results are not cached: %v", s.dir, s.failed)
}

// previous is the record a unit had before its text changed, if one is kept.
func (s *cacheStore) previous(u *cacheUnit) (unitRecord, bool) {
	prefix := unitPrefix + unitStem(u.rel) + "-"
	for _, e := range s.entries() {
		var rec unitRecord
		if strings.HasPrefix(e.Name(), prefix) && s.read(e.Name(), &rec) && rec.API == u.apiKey && rec.covers(u) {
			return rec, true
		}
	}
	return unitRecord{}, false
}

// entries lists the directory once per run.
func (s *cacheStore) entries() []os.DirEntry {
	if !s.listed {
		s.listing, _ = os.ReadDir(s.dir)
		s.listed = true
	}
	return s.listing
}

// touch marks a record as used now, so that eviction drops the least recently
// used first. A directory that cannot be written keeps its old times.
func (s *cacheStore) touch(name string) {
	now := time.Now()
	_ = os.Chtimes(filepath.Join(s.dir, name), now, now)
}

// sweep removes the records of units that are gone or have a newer record, and
// then the least recently used records while the directory is over its limit.
// It assumes the host sends the whole project in every request: a record of a
// unit that is not in this request is taken to belong to a deleted unit.
// Temporary files left by an interrupted write go after an hour.
func (s *cacheStore) sweep(current map[string]string) {
	if s.failed != nil {
		return
	}
	entries, err := os.ReadDir(s.dir)
	if err != nil {
		return
	}
	type record struct {
		name string
		size int64
		time int64
	}
	var kept []record
	var total int64
	for _, e := range entries {
		name := e.Name()
		if strings.HasPrefix(name, ".tmp-") {
			if info, err := e.Info(); err == nil && time.Since(info.ModTime()) > time.Hour {
				_ = os.Remove(filepath.Join(s.dir, name))
			}
			continue
		}
		if !strings.HasPrefix(name, unitPrefix) {
			continue
		}
		stem, _, _ := strings.Cut(strings.TrimPrefix(name, unitPrefix), "-")
		if want, known := current[stem]; !known || !strings.Contains(name, "-"+want+".json") {
			_ = os.Remove(filepath.Join(s.dir, name))
			continue
		}
		info, err := e.Info()
		if err != nil {
			continue
		}
		kept = append(kept, record{name, info.Size(), info.ModTime().UnixNano()})
		total += info.Size()
	}
	sort.Slice(kept, func(i, j int) bool { return kept[i].time < kept[j].time })
	for _, rec := range kept {
		if total <= cacheLimit {
			return
		}
		if os.Remove(filepath.Join(s.dir, rec.name)) == nil {
			total -= rec.size
		}
	}
}

// implementsOf decodes the `implements` edges of each file of the record.
func (rec unitRecord) implementsOf() map[string][]sdk.Edge {
	out := map[string][]sdk.Edge{}
	for _, f := range rec.Files {
		var frag struct {
			Edges []sdk.Edge `json:"edges"`
		}
		if json.Unmarshal(f.Fragment, &frag) != nil {
			continue
		}
		out[f.Path] = slices.DeleteFunc(frag.Edges, func(e sdk.Edge) bool { return e.Kind != edgeImplements })
	}
	return out
}

// covers reports whether the record holds exactly the requested files of u.
func (rec unitRecord) covers(u *cacheUnit) bool {
	if len(rec.Files) != len(u.requested) {
		return false
	}
	for _, f := range rec.Files {
		if !slices.Contains(u.requested, f.Path) {
			return false
		}
	}
	return true
}

// farOf is the methods of the file that only far interfaces give a role.
func (rec unitRecord) farOf(rel string) []string {
	for _, f := range rec.Files {
		if f.Path == rel {
			return f.Far
		}
	}
	return nil
}

// unitName is the file of the record of a unit at a key. Records of one unit
// share a prefix, so a new one can replace the old.
func unitName(unit, key string) string {
	return unitPrefix + unitStem(unit) + "-" + key + ".json"
}

func unitStem(unit string) string {
	return digest(unit)[:16]
}

// reuseImplements puts what the analysis of the missed units alone could not
// see where it found only its own: the `implements` edges of the replaced
// records, and the `implementation` role of the methods that only an interface
// of a package the unit does not import gives. The declarations of the unit and
// of everything else are unchanged, so the old ones are what a full analysis
// would give.
func reuseImplements(sub *run, plan *cachePlan) {
	for _, u := range plan.misses {
		previous := plan.previous[u]
		old := previous.implementsOf()
		for _, rel := range u.requested {
			frag := sub.fragments[rel]
			if frag == nil {
				continue
			}
			edges := slices.DeleteFunc(slices.Clone(frag.Edges), func(e sdk.Edge) bool { return e.Kind == edgeImplements })
			frag.Edges = append(edges, old[rel]...)
			far := previous.farOf(rel)
			sub.far[rel] = far
			for i := range frag.Functions {
				if slices.Contains(far, frag.Functions[i].Symbol) {
					frag.Functions[i].Implementation = true
				}
			}
		}
	}
}

// install puts the cached fragments and the analysis of the misses into r.
func install(r *run, plan *cachePlan, sub *run) {
	if sub != nil {
		r.result.Incomplete = append(r.result.Incomplete, sub.result.Incomplete...)
	}
	for _, rec := range plan.hits {
		for _, f := range rec.Files {
			r.encoded[f.Path] = f.Fragment
			if slices.Contains(rec.Excluded, f.Path) {
				r.excludedFiles[f.Path] = true
			} else {
				r.claimed[f.Path] = true
			}
		}
	}
	for _, u := range plan.misses {
		for _, rel := range u.requested {
			r.fragments[rel] = sub.fragments[rel]
			r.claimed[rel] = sub.claimed[rel]
			r.excludedFiles[rel] = sub.excludedFiles[rel]
			if reason, bad := sub.problems[rel]; bad {
				r.problems[rel] = reason
			}
		}
	}
}

// recordOf is the record of an analyzed unit; none when any of its files had a
// problem, as only complete results are cached.
func recordOf(u *cacheUnit, sub *run, times *timings) (unitRecord, bool) {
	rec := unitRecord{API: u.apiKey, Files: []recordFile{}, Excluded: []string{}}
	for _, rel := range u.requested {
		frag := sub.fragments[rel]
		if _, bad := sub.problems[rel]; bad || frag == nil {
			return unitRecord{}, false
		}
		encoded := times.measure(phaseSerialise)
		data, err := json.Marshal(frag)
		encoded()
		if err != nil {
			return unitRecord{}, false
		}
		rec.Files = append(rec.Files, recordFile{Path: rel, Fragment: data, Far: sub.far[rel]})
		if !sub.claimed[rel] {
			if !sub.excludedFiles[rel] {
				return unitRecord{}, false
			}
			rec.Excluded = append(rec.Excluded, rel)
		}
	}
	return rec, true
}
