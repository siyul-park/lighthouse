package provider

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

const (
	// cacheSchema versions the layout and meaning of everything in the cache
	// directory; changing it makes every entry a miss.
	cacheSchema = "lang-go-cache/1"
	// cacheLimit caps the size of the cache directory; the oldest entries go first.
	cacheLimit = 256 << 20

	unitPrefix = "u-"
	factsFile  = "facts.json"
)

// cacheStore is the directory the host lets the provider keep results in.
// Every read problem is a miss; the first write problem is remembered and
// stops further writes.
type cacheStore struct {
	dir    string
	failed error
}

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

// unitName is the file of the record of a unit at a key. Records of one unit
// share a prefix, so a new one can replace the old.
func unitName(unit, key string) string {
	return unitPrefix + unitStem(unit) + "-" + key + ".json"
}

func unitStem(unit string) string { return digest(unit)[:16] }

// previous is the record a unit had before its text changed, if one is kept.
func (s *cacheStore) previous(u *cacheUnit) (unitRecord, bool) {
	entries, err := os.ReadDir(s.dir)
	if err != nil {
		return unitRecord{}, false
	}
	prefix := unitPrefix + unitStem(u.rel) + "-"
	for _, e := range entries {
		var rec unitRecord
		if strings.HasPrefix(e.Name(), prefix) && s.read(e.Name(), &rec) && rec.API == u.apiKey && rec.covers(u) {
			return rec, true
		}
	}
	return unitRecord{}, false
}

// sweep removes the records of units that are gone or have a newer record, and
// then the oldest records while the directory is over its limit.
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
