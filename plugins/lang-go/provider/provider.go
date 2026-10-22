// Package provider is the Go language provider: it loads packages with
// go/packages, type-checks them with go/types and reports the code model of
// docs/plugin-protocol.md.
package provider

import "github.com/siyul-park/lighthouse/plugins/lang-go/sdk"

// Provider is the language provider of Go; it implements sdk.Handler.
type Provider struct {
	id      string
	version string
	last    RunStats
}

const language = "go"

// Initialize describes the Go language.
func (p *Provider) Initialize(sdk.InitializeParams) (sdk.InitializeResult, error) {
	return sdk.InitializeResult{
		ProtocolVersion: sdk.ProtocolVersion,
		ServerInfo:      sdk.Implementation{Name: p.id, Version: p.version},
		Capabilities: sdk.ServerCapabilities{Languages: []sdk.ProviderManifest{{
			ID:           language,
			Globs:        []string{"**/*.go"},
			Conventions:  sdk.Conventions{TestGlobs: []string{"**/*_test.go"}, ConstructorPrefixes: []string{"New", "new"}},
			Capabilities: []string{sdk.SemanticEdges, sdk.Extent, sdk.ReferenceSites, sdk.Overlays},
		}}},
	}, nil
}

// Index analyzes every requested file. Failures are reported as incomplete
// entries; a bad request never ends the process.
func (p *Provider) Index(params sdk.IndexParams) (sdk.IndexResult, error) {
	opts, err := newOptions(params.Context.Options)
	if err != nil {
		result := emptyResult()
		result.Incomplete = append(result.Incomplete, sdk.Incomplete{Reason: err.Error()})
		return result, nil
	}
	r := newRun(params, opts)
	p.last = RunStats{}
	if !p.indexCached(r, params) {
		r = newRun(params, opts)
		r.index()
	}
	return r.finish(), nil
}

// Stats says which units the last index request read from the cache and which
// it analyzed; both are empty when the request ran without the cache. Tests
// use it to assert what an edit re-indexes.
func (p *Provider) Stats() RunStats { return p.last }

// New returns a Go provider that identifies itself as id and version.
func New(id, version string) *Provider { return &Provider{id: id, version: version} }
