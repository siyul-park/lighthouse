// Package sdk speaks the Lighthouse plugin protocol: LSP-framed JSON-RPC 2.0
// and the wire types of docs/plugin-protocol.md. It knows nothing about
// languages or Lighthouse rules.
package sdk

import "encoding/json"

// ClientInfo identifies the host in InitializeParams.
type ClientInfo struct {
	Name    string `json:"name"`
	Version string `json:"version"`
}

// InitializeParams is the request of the `initialize` method.
type InitializeParams struct {
	Root            string     `json:"root"`
	ProtocolVersion string     `json:"protocolVersion"`
	ClientInfo      ClientInfo `json:"clientInfo"`
}

// InitializeResult is the response of the `initialize` method.
type InitializeResult struct {
	ID              string     `json:"id"`
	Version         string     `json:"version"`
	ProtocolVersion string     `json:"protocolVersion"`
	Languages       []Language `json:"languages"`
}

// Language is one language a plugin serves and how files select it.
type Language struct {
	ID           string      `json:"id"`
	Globs        []string    `json:"globs"`
	Priority     int         `json:"priority"`
	Fallback     bool        `json:"fallback,omitempty"`
	Conventions  Conventions `json:"conventions"`
	Capabilities []string    `json:"capabilities"`
}

// Conventions are the file conventions a language declares for the whole host.
type Conventions struct {
	TestGlobs []string `json:"test_globs"`
}

// ProjectRef locates the project an index request refers to.
type ProjectRef struct {
	Root string `json:"root"`
}

// FileRef names a project-relative file and its content hash.
type FileRef struct {
	Path string `json:"path"`
	Hash string `json:"hash"`
}

// Overlay is reserved; hosts do not send overlays in protocol 0.1.
type Overlay struct {
	Path string `json:"path"`
	Text string `json:"text"`
}

// Context carries the per-language options of lighthouse.toml.
type Context struct {
	Options  map[string]json.RawMessage `json:"options"`
	Overlays []Overlay                  `json:"overlays,omitempty"`
}

// IndexParams is the request of the `index` method.
type IndexParams struct {
	Project  ProjectRef `json:"project"`
	Language string     `json:"language"`
	Files    []FileRef  `json:"files"`
	Context  Context    `json:"context"`
}

// IndexResult is the response of the `index` method. Its lists must encode as
// `[]` when empty, never `null`.
type IndexResult struct {
	Fragments  []Fragment   `json:"fragments"`
	Notices    []string     `json:"notices"`
	Incomplete []Incomplete `json:"incomplete"`
}

// Incomplete reports why a file, or the whole request when Path is empty, was
// not fully analyzed.
type Incomplete struct {
	Path   string `json:"path,omitempty"`
	Reason string `json:"reason"`
}

// Fragment is the code model of one file.
type Fragment struct {
	File      FileInfo          `json:"file"`
	Modules   []Module          `json:"modules"`
	Symbols   []Symbol          `json:"symbols"`
	Edges     []Edge            `json:"edges"`
	Functions []FunctionSummary `json:"functions"`
	Tests     []TestCase        `json:"tests"`
}

// FileInfo identifies the file of a Fragment.
type FileInfo struct {
	Path      string `json:"path"`
	Generated bool   `json:"generated,omitempty"`
}

// Module is a unit of dependency such as a package or directory.
type Module struct {
	Path   string `json:"path"`
	Name   string `json:"name,omitempty"`
	TestOf string `json:"test_of,omitempty"`
}

// Position is a one-based line and column.
type Position struct {
	Line int `json:"line"`
	Col  int `json:"col"`
}

// Span is a source range.
type Span struct {
	Start Position `json:"start"`
	End   Position `json:"end"`
}

// Symbol is a declared name with its kind, owner and location.
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

// Edge is a relation from a Node to a target symbol or module.
type Edge struct {
	Kind       string `json:"kind"`
	From       Node   `json:"from"`
	To         string `json:"to"`
	Resolution string `json:"resolution"`
}

// Flow is one normalized control-flow event of a function body.
type Flow struct {
	Kind      string `json:"kind"`
	Nesting   int    `json:"nesting"`
	Arms      int    `json:"arms,omitempty"`
	Operators int    `json:"operators,omitempty"`
	Returning bool   `json:"returning,omitempty"`
}

// FunctionSummary is the measured shape of one function or method.
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

// TestCase describes how a test entry point is written and what it targets.
type TestCase struct {
	Symbol  string   `json:"symbol"`
	Nesting int      `json:"nesting"`
	Style   string   `json:"style"`
	Targets []string `json:"targets"`
}

// ProtocolVersion is the wire protocol version this package implements.
const ProtocolVersion = "0.1"

// SemanticEdges names the capability of reporting type-resolved edges.
const SemanticEdges = "semantic-edges"
