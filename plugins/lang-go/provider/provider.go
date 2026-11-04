// Package provider is the Go language provider: it loads packages with
// go/packages, type-checks them with go/types and reports the code model of
// docs/plugin-protocol.md.
package provider

import "github.com/siyul-park/lighthouse/plugins/lang-go/sdk"

// Provider is the language provider of Go; it implements sdk.Provider.
type Provider struct {
	id      string
	version string
	last    RunStats
}

const language = "go"

// New returns a Go provider that identifies itself as id and version.
func New(id, version string) *Provider {
	return &Provider{id: id, version: version}
}

// Name is the plugin id.
func (p *Provider) Name() string {
	return p.id
}

// Version is the plugin version.
func (p *Provider) Version() string {
	return p.version
}

// Manifest describes the Go language.
func (p *Provider) Manifest() []sdk.LanguageManifest {
	return []sdk.LanguageManifest{{
		ID:      language,
		Aliases: []string{"go", "golang"},
		Globs:   []string{"**/*.go"},
		// The go command ignores directories whose name starts with `.` or `_`
		// (`go help packages`); `.lighthouse` holds the decisions of the project.
		Ignores: []string{"_*/", ".*/", "!/.lighthouse/"},
		Conventions: sdk.Conventions{
			TestGlobs:           []string{"**/*_test.go"},
			ConstructorPrefixes: []string{"New", "new", "MustNew", "mustNew"},
			TestNaming:          sdk.TestNaming{Prefix: "Test", Variants: true},
		},
		Capabilities: []string{sdk.SemanticEdges, sdk.Extent, sdk.ReferenceSites, sdk.Overlays, "source-ast", "cache-overlays"},
	}}
}

// Index analyzes every requested file, and the regions of host files, which
// are loaded with them: what the go command loads for one it loads for both.
// Failures are reported as incomplete entries; a bad request never ends the
// process.
func (p *Provider) Index(params sdk.IndexParams) (sdk.IndexResult, error) {
	opts, err := newOptions(params.Context.Options)
	if err != nil {
		result := emptyResult()
		result.Incomplete = append(result.Incomplete, sdk.Incomplete{Reason: err.Error()})
		return result, nil
	}
	r := newRun(params, opts)
	p.last = RunStats{}
	snippets := r.fences(params)
	astRequested := false
	for _, input := range params.Inputs {
		if input.Kind == "file" && input.File != nil && input.SourceAST {
			astRequested = true
			break
		}
	}
	var analysed *run
	cached := false
	if !astRequested {
		analysed, cached = p.indexCached(r, params, snippets)
	}
	if !cached {
		r = newRun(params, opts)
		r.admit(snippets)
		r.index()
		p.last = RunStats{Loads: r.loads}
		analysed = r
	}
	result := r.finish()
	if len(snippets) > 0 {
		result = analysed.regionResult(result, snippets)
	}
	return result, nil
}

// Stats says which units the last index request read from the cache and which
// it analyzed; both are empty when the request ran without the cache. Tests
// use it to assert what an edit re-indexes.
func (p *Provider) Stats() RunStats {
	return p.last
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
