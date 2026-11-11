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

// RunStats says what the last index request cost.
type RunStats struct {
	// Loads is the number of batches the go command loaded.
	Loads int
	// Packages is the number of packages they gave.
	Packages int
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
		Capabilities: []string{sdk.SemanticEdges, sdk.Extent, sdk.ReferenceSites, sdk.Overlays, "source-ast", "steps"},
		Units: sdk.Units{
			By: "directory",
			// What decides how the go command reads every package.
			Settings: []string{"go.mod", "go.sum", "go.work", "go.work.sum"},
			Env:      goEnvVars,
		},
	}}
}

// Stats reports what the last index request cost.
func (p *Provider) Stats() RunStats {
	return p.last
}

// Index analyzes the files of the units asked for, and the regions of host
// files, which are loaded with them: what the go command loads for one it
// loads for both. Failures are reported as incomplete entries; a bad request
// never ends the process.
func (p *Provider) Index(params sdk.IndexParams) (sdk.IndexResult, error) {
	opts, err := newOptions(params.Context.Options)
	if err != nil {
		result := emptyResult()
		result.Incomplete = append(result.Incomplete, sdk.Incomplete{Reason: err.Error()})
		return result, nil
	}
	r := newRun(params, opts)
	snippets := r.fences(params)
	planned := r.times.measure(phasePlan)
	shape := r.plan(params)
	planned()
	r.admit(snippets)
	r.index()
	p.last = RunStats{Loads: r.loads, Packages: r.packages}
	found := r.finish()
	result := emptyResult()
	result.Units = r.unitResults(shape, params, snippets, found)
	result.Notices = append(result.Notices, found.Notices...)
	for _, gap := range found.Incomplete {
		if gap.Path == "" {
			result.Incomplete = append(result.Incomplete, gap)
		}
	}
	for _, ph := range r.times.phases() {
		result.Timings = append(result.Timings, sdk.PhaseTime{Phase: ph.name, Micros: int(ph.spent.Microseconds())})
	}
	return result, nil
}
