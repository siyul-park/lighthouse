// Package provider is the Go language provider: it loads packages with
// go/packages, type-checks them with go/types and reports the code model of
// docs/plugin-protocol.md.
package provider

import (
	"fmt"
	"sort"

	"github.com/siyul-park/lighthouse/plugins/lang-go/sdk"
)

const language = "go"

// Provider implements sdk.Handler for Go.
type Provider struct {
	id      string
	version string
}

func New(id, version string) *Provider { return &Provider{id: id, version: version} }

func (p *Provider) Initialize(params sdk.InitializeParams) (sdk.InitializeResult, error) {
	if params.ProtocolVersion != sdk.ProtocolVersion {
		return sdk.InitializeResult{}, fmt.Errorf("host speaks protocol %s, this plugin speaks %s", params.ProtocolVersion, sdk.ProtocolVersion)
	}
	return sdk.InitializeResult{
		ID:              p.id,
		Version:         p.version,
		ProtocolVersion: sdk.ProtocolVersion,
		Languages: []sdk.Language{{
			ID:           language,
			Globs:        []string{"**/*.go"},
			Conventions:  sdk.Conventions{TestGlobs: []string{"**/*_test.go"}},
			Capabilities: []string{sdk.SemanticEdges},
		}},
	}, nil
}

// Index analyzes every requested file. Failures are reported as incomplete
// entries; a bad request never ends the process.
func (p *Provider) Index(params sdk.IndexParams) (sdk.IndexResult, error) {
	run := newRun(params)
	opts, err := parseOptions(params.Context.Options, language)
	if err == nil {
		opts, err = opts.resolve()
	}
	if err != nil {
		run.result.Incomplete = append(run.result.Incomplete, sdk.Incomplete{Reason: err.Error()})
		return run.result, nil
	}
	run.opts = opts
	run.index()
	return run.finish(), nil
}

func (r *run) finish() sdk.IndexResult {
	paths := make([]string, 0, len(r.fragments))
	for path := range r.fragments {
		paths = append(paths, path)
	}
	sort.Strings(paths)
	for _, path := range paths {
		r.result.Fragments = append(r.result.Fragments, *r.fragments[path])
	}
	for path, reason := range r.problems {
		r.result.Incomplete = append(r.result.Incomplete, sdk.Incomplete{Path: path, Reason: reason})
	}
	sort.Slice(r.result.Incomplete, func(i, j int) bool {
		a, b := r.result.Incomplete[i], r.result.Incomplete[j]
		return a.Path < b.Path || (a.Path == b.Path && a.Reason < b.Reason)
	})
	if r.excluded > 0 {
		r.result.Notices = append(r.result.Notices, fmt.Sprintf(
			"%d file(s) excluded by build constraints, e.g. %s; select another context with [languages.go] tags or env",
			r.excluded, r.excludedExample))
	}
	return r.result
}
