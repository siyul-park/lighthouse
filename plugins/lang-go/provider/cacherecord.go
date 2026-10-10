package provider

import (
	"encoding/json"
	"slices"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

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
}

// reuseImplements puts the `implements` edges of the replaced records where the
// analysis, which saw only the missed units, found some of its own. The
// declarations of the unit and of everything it imports are unchanged, so
// the old edges are the ones a full analysis would give.
func reuseImplements(sub *run, plan *cachePlan) {
	for _, u := range plan.misses {
		old := plan.previous[u].implementsOf()
		for _, rel := range u.requested {
			if frag := sub.fragments[rel]; frag != nil {
				edges := slices.DeleteFunc(slices.Clone(frag.Edges), func(e sdk.Edge) bool { return e.Kind == edgeImplements })
				frag.Edges = append(edges, old[rel]...)
			}
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

// record stores the units the analysis completed without a problem, and drops
// what the cache holds of units that changed or are gone.
func (p *Provider) record(store *cacheStore, plan *cachePlan, sub *run) {
	current := map[string]string{}
	for _, u := range plan.units {
		current[unitStem(u.rel)] = u.key
	}
	for _, u := range plan.misses {
		if u.cgo {
			continue
		}
		if rec, ok := recordOf(u, sub); ok {
			store.write(unitName(u.rel, u.key), rec)
		}
	}
	store.sweep(current)
}

// recordOf is the record of an analyzed unit; none when any of its files had a
// problem, as only complete results are cached.
func recordOf(u *cacheUnit, sub *run) (unitRecord, bool) {
	rec := unitRecord{API: u.apiKey, Files: []recordFile{}, Excluded: []string{}}
	for _, rel := range u.requested {
		frag := sub.fragments[rel]
		if _, bad := sub.problems[rel]; bad || frag == nil {
			return unitRecord{}, false
		}
		data, err := json.Marshal(frag)
		if err != nil {
			return unitRecord{}, false
		}
		rec.Files = append(rec.Files, recordFile{Path: rel, Fragment: data})
		if !sub.claimed[rel] {
			if !sub.excludedFiles[rel] {
				return unitRecord{}, false
			}
			rec.Excluded = append(rec.Excluded, rel)
		}
	}
	return rec, true
}

func (plan *cachePlan) stats() RunStats {
	var s RunStats
	for _, u := range plan.units {
		if _, hit := plan.hits[u]; hit {
			s.Hits = append(s.Hits, u.rel)
		} else {
			s.Misses = append(s.Misses, u.rel)
		}
	}
	return s
}
