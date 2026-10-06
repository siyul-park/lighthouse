package sdk

import (
	"bufio"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"strconv"
	"strings"
)

const maxBody = 256 << 20

// Message is one JSON-RPC 2.0 message: a request or notification (Method), or
// a response (Result or Error).
type Message struct {
	JSONRPC string           `json:"jsonrpc"`
	ID      *json.RawMessage `json:"id,omitempty"`
	Method  string           `json:"method,omitempty"`
	Params  json.RawMessage  `json:"params,omitempty"`
	Result  json.RawMessage  `json:"result,omitempty"`
	Error   *ResponseError   `json:"error,omitempty"`
}

type ResponseError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

// ReadMessage reads one Content-Length framed message. It returns io.EOF at a
// clean end of input.
func ReadMessage(r *bufio.Reader) (*Message, error) {
	length := -1
	first := true
	for {
		line, err := r.ReadString('\n')
		if err != nil {
			if err == io.EOF && first && line == "" {
				return nil, io.EOF
			}
			return nil, fmt.Errorf("sdk: reading header: %w", unexpected(err))
		}
		first = false
		line = strings.TrimRight(line, "\r\n")
		if line == "" {
			break
		}
		name, value, ok := strings.Cut(line, ":")
		if !ok {
			return nil, fmt.Errorf("sdk: malformed header %q", line)
		}
		if strings.EqualFold(name, "Content-Length") {
			n, err := strconv.Atoi(strings.TrimSpace(value))
			if err != nil || n < 0 {
				return nil, fmt.Errorf("sdk: malformed header %q", line)
			}
			length = n
		}
	}
	if length < 0 {
		return nil, errors.New("sdk: missing Content-Length")
	}
	if length > maxBody {
		return nil, fmt.Errorf("sdk: message of %d bytes is too large", length)
	}
	body := make([]byte, length)
	if _, err := io.ReadFull(r, body); err != nil {
		return nil, fmt.Errorf("sdk: reading body: %w", unexpected(err))
	}
	var m Message
	if err := json.Unmarshal(body, &m); err != nil {
		return nil, fmt.Errorf("sdk: malformed message: %w", err)
	}
	return &m, nil
}

// WriteMessage writes one Content-Length framed message.
func WriteMessage(w io.Writer, m *Message) error {
	m.JSONRPC = "2.0"
	body, err := json.Marshal(m)
	if err != nil {
		return err
	}
	if _, err := fmt.Fprintf(w, "Content-Length: %d\r\n\r\n", len(body)); err != nil {
		return err
	}
	_, err = w.Write(body)
	return err
}

// unexpected turns an end of input inside a message into an error that
// callers cannot mistake for a clean end of input.
func unexpected(err error) error {
	if err == io.EOF {
		return io.ErrUnexpectedEOF
	}
	return err
}
