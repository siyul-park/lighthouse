package sdk

import (
	"bufio"
	"encoding/json"
	"errors"
	"fmt"
	"io"
)

// Provider is a language provider: its identity, the languages it declares
// and how it indexes the files of one request.
type Provider interface {
	// Name is the plugin id.
	Name() string
	Version() string
	// Manifest lists the languages the provider answers `initialize` with.
	Manifest() []LanguageManifest
	Index(IndexParams) (IndexResult, error)
}

// session is what the loop knows of the conversation.
type session struct {
	initialized bool
	shutdown    bool
}

// Method names of the plugin protocol (crates/lighthouse-protocol).
const (
	MethodInitialize = "initialize"
	MethodIndex      = "lighthouse/index"
	MethodShutdown   = "shutdown"
	MethodExit       = "exit"
)

const (
	codeParse          = -32700
	codeMethod         = -32601
	codeParams         = -32602
	codeInternal       = -32603
	codeRequestFailed  = -32803 // LSP RequestFailed: a handler could not do the job
	codeNotInitialized = -32002 // LSP ServerNotInitialized: a request before `initialize`
)

// Serve answers requests from r on w until the host sends `exit` or closes
// the input. It refuses an `initialize` of another protocol version, and any
// other request before an `initialize` that succeeded. It returns nil after a
// `shutdown` followed by `exit`, and an error when the input ends or breaks
// first.
func Serve(r io.Reader, w io.Writer, p Provider) error {
	in := bufio.NewReader(r)
	var s session
	for {
		m, err := ReadMessage(in)
		if errors.Is(err, io.EOF) {
			return errors.New("sdk: input closed before exit")
		}
		if err != nil {
			_ = WriteMessage(w, &Message{Error: &ResponseError{Code: codeParse, Message: err.Error()}})
			return err
		}
		if m.Method == MethodExit {
			if s.shutdown {
				return nil
			}
			return errors.New("sdk: exit before shutdown")
		}
		if m.Method == "" {
			continue
		}
		result, rerr := dispatch(p, m, &s)
		if m.ID == nil {
			continue
		}
		reply := &Message{ID: m.ID}
		if rerr != nil {
			reply.Error = rerr
		} else {
			reply.Result = result
		}
		if err := WriteMessage(w, reply); err != nil {
			return err
		}
	}
}

func dispatch(p Provider, m *Message, s *session) (result json.RawMessage, rerr *ResponseError) {
	defer func() {
		if p := recover(); p != nil {
			result, rerr = nil, &ResponseError{Code: codeInternal, Message: fmt.Sprintf("panic: %v", p)}
		}
	}()
	switch {
	case m.Method == MethodInitialize:
		var params InitializeParams
		if rerr := decode(m.Params, &params, "protocolVersion", "clientInfo", "workspaceFolders"); rerr != nil {
			return nil, rerr
		}
		if params.ProtocolVersion != ProtocolVersion {
			return nil, &ResponseError{Code: codeRequestFailed, Message: fmt.Sprintf("host speaks protocol %s, this plugin speaks %s", params.ProtocolVersion, ProtocolVersion)}
		}
		result, rerr = marshal(initialize(p), nil)
		s.initialized = s.initialized || rerr == nil
		return result, rerr
	case !s.initialized:
		return nil, &ResponseError{Code: codeNotInitialized, Message: fmt.Sprintf("`%s` before `initialize`", m.Method)}
	case m.Method == MethodIndex:
		var params IndexParams
		if rerr := decode(m.Params, &params, "project", "language", "inputs"); rerr != nil {
			return nil, rerr
		}
		return marshal(p.Index(params))
	case m.Method == MethodShutdown:
		s.shutdown = true
		return json.RawMessage("null"), nil
	default:
		return nil, &ResponseError{Code: codeMethod, Message: "unknown method " + m.Method}
	}
}

// initialize is the answer to the handshake.
func initialize(p Provider) InitializeResult {
	return InitializeResult{
		ProtocolVersion: ProtocolVersion,
		ServerInfo:      Implementation{Name: p.Name(), Version: p.Version()},
		Capabilities:    ServerCapabilities{Languages: p.Manifest()},
	}
}

// decode reads params into v and refuses params that lack a required member,
// as the Rust server does for the fields its types require.
func decode(params json.RawMessage, v any, required ...string) *ResponseError {
	var members map[string]json.RawMessage
	if err := json.Unmarshal(params, &members); err != nil {
		return &ResponseError{Code: codeParams, Message: err.Error()}
	}
	for _, name := range required {
		if _, ok := members[name]; !ok {
			return &ResponseError{Code: codeParams, Message: "missing field `" + name + "`"}
		}
	}
	if err := json.Unmarshal(params, v); err != nil {
		return &ResponseError{Code: codeParams, Message: err.Error()}
	}
	return nil
}

func marshal[T any](value T, err error) (json.RawMessage, *ResponseError) {
	if err != nil {
		return nil, &ResponseError{Code: codeRequestFailed, Message: err.Error()}
	}
	body, err := json.Marshal(value)
	if err != nil {
		return nil, &ResponseError{Code: codeInternal, Message: err.Error()}
	}
	return body, nil
}
