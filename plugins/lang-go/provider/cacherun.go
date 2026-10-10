package provider

import (
	"path/filepath"
	"reflect"
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
	// previous holds, for a missed unit, the record it replaces when only the
	// unit's own text changed.
	previous map[*cacheUnit]unitRecord
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
	pl := &planner{r: r, used: map[string]fileFacts{}, units: map[string]*cacheUnit{}}
	version := digest(cacheSchema, p.identity())
	plan, ok := p.prepare(pl, store, version, params)
	if !ok {
		return false
	}
	sub, ok := analyzeMisses(r, params, plan)
	if !ok {
		return false
	}
	p.finish(r, store, plan, sub, pl)
	p.last = plan.stats()
	return true
}

// finish installs the answer into r and keeps what the analysis learned.
func (p *Provider) finish(r *run, store *cacheStore, plan *cachePlan, sub *run, pl *planner) {
	install(r, plan, sub)
	p.record(store, plan, sub)
	if !reflect.DeepEqual(pl.used, pl.facts) {
		store.write(factsFile, factsRecord{Version: digest(cacheSchema, p.identity()), Files: pl.used})
	}
	if notice := store.notice(); notice != "" {
		r.result.Notices = append(r.result.Notices, notice)
	}
	candidates, ignored := r.candidates()
	for _, rel := range ignored {
		r.fileOnly(rel)
	}
	for _, rel := range candidates {
		if !r.claimed[rel] {
			r.unclaimed(rel)
		}
	}
}

// prepare reads the environment and the cached facts and plans the units.
func (p *Provider) prepare(pl *planner, store *cacheStore, version string, params sdk.IndexParams) (*cachePlan, bool) {
	candidates, _ := pl.r.candidates()
	batches := pl.r.batches(candidates)
	if len(batches) == 0 {
		return nil, false
	}
	var stored factsRecord
	if store.read(factsFile, &stored) && stored.Version == version {
		pl.facts = stored.Files
	}
	env, modules, ok := pl.environment(batches)
	if !ok {
		return nil, false
	}
	pl.modules = modules
	return p.plan(pl, store, env, candidates, params)
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
	base := digest(cacheSchema, p.identity(), p.id, p.version, options, env, pl.interfaceDigest())
	plan := &cachePlan{units: pl.requestedUnits, hits: map[*cacheUnit]unitRecord{}, previous: map[*cacheUnit]unitRecord{}}
	for _, u := range plan.units {
		u.apiKey = unitAPIKey(base, u)
		u.key = digest(u.apiKey, u.content)
		var rec unitRecord
		if !u.cgo && store.read(unitName(u.rel, u.key), &rec) && rec.covers(u) {
			store.touch(unitName(u.rel, u.key))
			plan.hits[u] = rec
			if u.ifaces != "" {
				plan.interfaces = append(plan.interfaces, u)
			}
			continue
		}
		plan.misses = append(plan.misses, u)
		if old, ok := store.previous(u); ok {
			plan.previous[u] = old
		}
	}
	return plan, true
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

// unitAPIKey is the key of u under base without the bodies of its functions:
// its own API and the API of everything it imports.
func unitAPIKey(base string, u *cacheUnit) string {
	parts := []string{base, u.rel, u.api}
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

// analyzeMisses runs the uncached analysis over the missed units, plus the
// units that declare interfaces so that types are matched against all of them.
// It returns false when that run failed as a whole, which the cold run would
// report differently.
func analyzeMisses(r *run, params sdk.IndexParams, plan *cachePlan) (*run, bool) {
	if len(plan.misses) == 0 {
		return nil, true
	}
	units := plan.misses
	if len(plan.previous) < len(plan.misses) {
		units = slices.Concat(plan.misses, plan.interfaces)
	}
	wanted := map[string]bool{}
	for _, u := range units {
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
	if !sameFailures(r, sub) {
		return sub, false
	}
	if len(plan.previous) == len(plan.misses) {
		reuseImplements(sub, plan)
	}
	return sub, true
}

// sameFailures reports whether the analysis of the missed units failed the way
// a full one would have. A batch the go command cannot list is reported with
// its number of files, so the analysis must have seen all of them; any other
// request-wide problem makes it differ.
func sameFailures(r, sub *run) bool {
	if len(sub.result.Incomplete) != len(sub.failed) {
		return false
	}
	candidates, _ := r.candidates()
	for _, b := range r.batches(candidates) {
		if n, failed := sub.failed[b.dir]; failed && n != len(b.files) {
			return false
		}
	}
	return true
}
