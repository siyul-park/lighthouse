package provider

import (
	"path/filepath"
	"slices"
	"sort"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

// RunStats says which units the last index request took from the cache and
// which it analyzed, by project-relative directory.
type RunStats struct {
	Hits   []string
	Misses []string
}

// unitRecord is what the cache keeps of a unit: the fragments of its requested
// files, and which of them the build constraints excluded.
type unitRecord struct {
	Fragments []sdk.Fragment `json:"fragments"`
	Excluded  []string       `json:"excluded"`
}

// factsRecord is the cached facts of source files by content hash.
type factsRecord struct {
	Version string               `json:"version"`
	Files   map[string]fileFacts `json:"files"`
}

// cachePlan is the units of a request and what the cache knows of them.
type cachePlan struct {
	units  []*cacheUnit
	hits   map[*cacheUnit]unitRecord
	misses []*cacheUnit
	// interfaces are the hit units that declare interfaces: the analysis of the
	// misses needs them loaded to match types against them.
	interfaces []*cacheUnit
}

// indexCached answers the request from the cache where it can and analyzes
// only the units it cannot. It changes r only when it answers; false means the
// caller must run uncached.
func (p *Provider) indexCached(r *run, params sdk.IndexParams) bool {
	if params.Context.Cache == nil || params.Context.Cache.Dir == "" || len(params.Context.Overlays) > 0 {
		return false
	}
	store := &cacheStore{dir: params.Context.Cache.Dir}
	candidates, ignored := r.candidates()
	batches := r.batches(candidates)
	if len(batches) == 0 {
		return false
	}
	version := digest(cacheSchema, executableHash())
	pl := &planner{r: r, used: map[string]fileFacts{}, units: map[string]*cacheUnit{}}
	var stored factsRecord
	if store.read(factsFile, &stored) && stored.Version == version {
		pl.facts = stored.Files
	}
	env, modules, ok := pl.environment(batches)
	if !ok {
		return false
	}
	pl.modules = modules
	plan, ok := p.plan(pl, store, env, candidates, params)
	if !ok {
		return false
	}
	sub, ok := analyzeMisses(r, params, plan)
	if !ok {
		return false
	}
	for _, rel := range ignored {
		r.fileOnly(rel)
	}
	install(r, plan, sub)
	p.record(store, plan, sub)
	store.write(factsFile, factsRecord{Version: version, Files: pl.used})
	if notice := store.notice(); notice != "" {
		r.result.Notices = append(r.result.Notices, notice)
	}
	for _, rel := range candidates {
		if !r.claimed[rel] {
			r.unclaimed(rel)
		}
	}
	p.last = plan.stats()
	return true
}

// candidates are the requested files a provider analyzes, sorted, and the
// ignored ones.
func (r *run) candidates() (candidates, ignored []string) {
	for rel := range r.requested {
		if ignoredPath(rel) {
			ignored = append(ignored, rel)
			continue
		}
		candidates = append(candidates, rel)
	}
	sort.Strings(candidates)
	sort.Strings(ignored)
	return candidates, ignored
}

// plan keys every unit of the candidates and looks it up.
func (p *Provider) plan(pl *planner, store *cacheStore, env string, candidates []string, params sdk.IndexParams) (*cachePlan, bool) {
	for _, rel := range candidates {
		if pl.unit(filepath.Dir(filepath.Join(pl.r.root, filepath.FromSlash(rel)))) == nil {
			return nil, false
		}
	}
	for _, u := range pl.units {
		if len(u.requested) > 0 {
			pl.requestedUnits = append(pl.requestedUnits, u)
		}
	}
	sort.Slice(pl.requestedUnits, func(i, j int) bool { return pl.requestedUnits[i].rel < pl.requestedUnits[j].rel })
	options := string(params.Context.Options[language])
	base := digest(cacheSchema, executableHash(), p.id, p.version, options, env, pl.interfaceDigest())
	plan := &cachePlan{units: pl.requestedUnits, hits: map[*cacheUnit]unitRecord{}}
	for _, u := range plan.units {
		u.key = unitKey(base, u)
		var rec unitRecord
		if store.read(unitName(u.rel, u.key), &rec) && rec.covers(u) {
			plan.hits[u] = rec
			if u.ifaces != "" {
				plan.interfaces = append(plan.interfaces, u)
			}
			continue
		}
		plan.misses = append(plan.misses, u)
	}
	return plan, true
}

// unitKey is the key of u under base: its own files and the API of everything
// it imports.
func unitKey(base string, u *cacheUnit) string {
	parts := []string{base, u.rel, u.content}
	for _, d := range closure(u) {
		parts = append(parts, d.rel, d.api)
	}
	return digest(parts...)
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

// covers reports whether the record holds exactly the requested files of u.
func (rec unitRecord) covers(u *cacheUnit) bool {
	if len(rec.Fragments) != len(u.requested) {
		return false
	}
	for _, f := range rec.Fragments {
		if !slices.Contains(u.requested, f.File.Path) {
			return false
		}
	}
	return true
}

// analyzeMisses runs the uncached analysis over the missed units, plus the
// units that declare interfaces so that types are matched against all of them.
// It returns false when that run failed as a whole, which the cold run would
// report differently.
func analyzeMisses(r *run, params sdk.IndexParams, plan *cachePlan) (*run, bool) {
	if len(plan.misses) == 0 {
		return nil, true
	}
	wanted := map[string]bool{}
	for _, u := range slices.Concat(plan.misses, plan.interfaces) {
		for _, rel := range u.requested {
			wanted[rel] = true
		}
	}
	subParams := params
	subParams.Files = nil
	for _, f := range params.Files {
		if wanted[f.Path] {
			subParams.Files = append(subParams.Files, f)
		}
	}
	sub := newRun(subParams, r.opts)
	sub.index()
	return sub, len(sub.result.Incomplete) == 0
}

// install puts the cached fragments and the analysis of the misses into r.
func install(r *run, plan *cachePlan, sub *run) {
	for _, rec := range plan.hits {
		excluded := map[string]bool{}
		for _, rel := range rec.Excluded {
			excluded[rel] = true
		}
		for i := range rec.Fragments {
			frag := rec.Fragments[i]
			rel := frag.File.Path
			r.fragments[rel] = &frag
			if excluded[rel] {
				r.excludedFiles[rel] = true
			} else {
				r.claimed[rel] = true
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
		if rec, ok := recordOf(u, sub); ok {
			store.write(unitName(u.rel, u.key), rec)
		}
	}
	store.sweep(current)
}

// recordOf is the record of an analyzed unit; none when any of its files had a
// problem, as only complete results are cached.
func recordOf(u *cacheUnit, sub *run) (unitRecord, bool) {
	rec := unitRecord{Fragments: []sdk.Fragment{}, Excluded: []string{}}
	for _, rel := range u.requested {
		if _, bad := sub.problems[rel]; bad || sub.fragments[rel] == nil {
			return unitRecord{}, false
		}
		rec.Fragments = append(rec.Fragments, *sub.fragments[rel])
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
