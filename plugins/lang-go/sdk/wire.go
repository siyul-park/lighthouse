// Package sdk speaks the Lighthouse plugin protocol: LSP-framed JSON-RPC 2.0
// and the wire types of docs/plugin-protocol.md. It knows nothing about
// languages or Lighthouse rules.
package sdk

import "encoding/json"

// ProtocolVersion is the wire protocol version this package implements.
const ProtocolVersion = "0.1"

// Capability names a provider feature.
const (
	SemanticEdges = "semantic-edges"
)

type ClientInfo struct {
	Name    string `json:"name"`
	Version string `json:"version"`
}

type InitializeParams struct {
	Root            string     `json:"root"`
	ProtocolVersion string     `json:"protocolVersion"`
	ClientInfo      ClientInfo `json:"clientInfo"`
}

type InitializeResult struct {
	ID              string     `json:"id"`
	Version         string     `json:"version"`
	ProtocolVersion string     `json:"protocolVersion"`
	Languages       []Language `json:"languages"`
}

type Language struct {
	ID           string      `json:"id"`
	Globs        []string    `json:"globs"`
	Priority     int         `json:"priority"`
	Fallback     bool        `json:"fallback,omitempty"`
	Conventions  Conventions `json:"conventions"`
	Capabilities []string    `json:"capabilities"`
}

type Conventions struct {
	TestGlobs []string `json:"test_globs"`
}

type ProjectRef struct {
	Root string `json:"root"`
}

type FileRef struct {
	Path string `json:"path"`
	Hash string `json:"hash"`
}

// Overlay is reserved; hosts do not send overlays in protocol 0.1.
type Overlay struct {
	Path string `json:"path"`
	Text string `json:"text"`
}

type Context struct {
	Options  map[string]json.RawMessage `json:"options"`
	Overlays []Overlay                  `json:"overlays,omitempty"`
}

type IndexParams struct {
	Project  ProjectRef `json:"project"`
	Language string     `json:"language"`
	Files    []FileRef  `json:"files"`
	Context  Context    `json:"context"`
}

type IndexResult struct {
	Fragments  []Fragment   `json:"fragments"`
	Notices    []string     `json:"notices"`
	Incomplete []Incomplete `json:"incomplete"`
}

type Incomplete struct {
	Path   string `json:"path,omitempty"`
	Reason string `json:"reason"`
}

type Fragment struct {
	File      FileInfo          `json:"file"`
	Modules   []Module          `json:"modules"`
	Symbols   []Symbol          `json:"symbols"`
	Edges     []Edge            `json:"edges"`
	Functions []FunctionSummary `json:"functions"`
	Tests     []TestCase        `json:"tests"`
}

type FileInfo struct {
	Path      string `json:"path"`
	Generated bool   `json:"generated,omitempty"`
}

type Module struct {
	Path   string `json:"path"`
	Name   string `json:"name,omitempty"`
	TestOf string `json:"test_of,omitempty"`
}

type Position struct {
	Line int `json:"line"`
	Col  int `json:"col"`
}

type Span struct {
	Start Position `json:"start"`
	End   Position `json:"end"`
}

type Symbol struct {
	ID         string `json:"id"`
	Kind       string `json:"kind"`
	Visibility string `json:"visibility"`
	Owner      string `json:"owner,omitempty"`
	File       string `json:"file"`
	Span       Span   `json:"span"`
	Doc        string `json:"doc,omitempty"`
	Name       string `json:"name"`
}

// Node is a module or a symbol: exactly one field is set.
type Node struct {
	Module string `json:"module,omitempty"`
	Symbol string `json:"symbol,omitempty"`
}

type Edge struct {
	Kind       string `json:"kind"`
	From       Node   `json:"from"`
	To         string `json:"to"`
	Resolution string `json:"resolution"`
}

type Flow struct {
	Kind      string `json:"kind"`
	Nesting   int    `json:"nesting"`
	Arms      int    `json:"arms,omitempty"`
	Operators int    `json:"operators,omitempty"`
	Returning bool   `json:"returning,omitempty"`
}

type FunctionSummary struct {
	Symbol           string `json:"symbol"`
	MaxNesting       int    `json:"max_nesting"`
	Statements       int    `json:"statements"`
	TopLevel         int    `json:"top_level"`
	Params           int    `json:"params"`
	Returns          int    `json:"returns"`
	Tokens           int    `json:"tokens"`
	Flow             []Flow `json:"flow"`
	CloneFingerprint string `json:"clone_fingerprint,omitempty"`
	ForwardsTo       string `json:"forwards_to,omitempty"`
}

type TestCase struct {
	Symbol  string   `json:"symbol"`
	Nesting int      `json:"nesting"`
	Style   string   `json:"style"`
	Targets []string `json:"targets"`
}
