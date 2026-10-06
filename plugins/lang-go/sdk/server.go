package sdk

import (
	"bufio"
	"encoding/json"
	"errors"
	"fmt"
	"io"
)

const (
	codeParse    = -32700
	codeMethod   = -32601
	codeParams   = -32602
	codeInternal = -32603
)

// Handler implements the methods of a language provider.
type Handler interface {
	Initialize(InitializeParams) (InitializeResult, error)
	Index(IndexParams) (IndexResult, error)
}

// Serve answers requests from r on w until the host sends `exit` or closes
// the input. It returns nil after a `shutdown` followed by `exit`, and an
// error when the input ends or breaks first.
func Serve(r io.Reader, w io.Writer, h Handler) error {
	in := bufio.NewReader(r)
	shutdown := false
	for {
		m, err := ReadMessage(in)
		if errors.Is(err, io.EOF) {
			return errors.New("sdk: input closed before exit")
		}
		if err != nil {
			_ = WriteMessage(w, &Message{Error: &ResponseError{Code: codeParse, Message: err.Error()}})
			return err
		}
		if m.Method == "exit" {
			if shutdown {
				return nil
			}
			return errors.New("sdk: exit before shutdown")
		}
		if m.Method == "" {
			continue
		}
		result, rerr := dispatch(h, m, &shutdown)
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

func dispatch(h Handler, m *Message, shutdown *bool) (result json.RawMessage, rerr *ResponseError) {
	defer func() {
		if p := recover(); p != nil {
			result, rerr = nil, &ResponseError{Code: codeInternal, Message: fmt.Sprintf("panic: %v", p)}
		}
	}()
	switch m.Method {
	case "initialize":
		var p InitializeParams
		if err := json.Unmarshal(m.Params, &p); err != nil {
			return nil, &ResponseError{Code: codeParams, Message: err.Error()}
		}
		return reply(h.Initialize(p))
	case "index":
		var p IndexParams
		if err := json.Unmarshal(m.Params, &p); err != nil {
			return nil, &ResponseError{Code: codeParams, Message: err.Error()}
		}
		return reply(h.Index(p))
	case "shutdown":
		*shutdown = true
		return json.RawMessage("null"), nil
	default:
		return nil, &ResponseError{Code: codeMethod, Message: "unknown method " + m.Method}
	}
}

func reply[T any](value T, err error) (json.RawMessage, *ResponseError) {
	if err != nil {
		return nil, &ResponseError{Code: codeInternal, Message: err.Error()}
	}
	body, err := json.Marshal(value)
	if err != nil {
		return nil, &ResponseError{Code: codeInternal, Message: err.Error()}
	}
	return body, nil
}
