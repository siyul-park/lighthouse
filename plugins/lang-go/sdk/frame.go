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

// ResponseError is the error object of a JSON-RPC response.
type ResponseError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

const maxBody = 256 << 20

// ReadMessage reads one Content-Length framed message. It returns io.EOF at a
// clean end of input.
func ReadMessage(r *bufio.Reader) (*Message, error) {
	length, err := readLength(r)
	if err != nil {
		return nil, err
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

// WriteMessage writes one Content-Length framed message in a single write.
// A Result is already encoded, so it is spliced into the encoded envelope
// instead of being encoded, and checked, a second time.
func WriteMessage(w io.Writer, m *Message) error {
	m.JSONRPC = "2.0"
	result := m.Result
	m.Result = nil
	envelope, err := json.Marshal(m)
	m.Result = result
	if err != nil {
		return err
	}
	const header = "Content-Length: "
	const separator = "\r\n\r\n"
	const field = `,"result":`
	size := len(envelope)
	if len(result) > 0 {
		size += len(field) + len(result)
	}
	frame := make([]byte, 0, len(header)+len(separator)+10+size)
	frame = append(frame, header...)
	frame = strconv.AppendInt(frame, int64(size), 10)
	frame = append(frame, separator...)
	if len(result) == 0 {
		frame = append(frame, envelope...)
	} else {
		frame = append(frame, envelope[:len(envelope)-1]...)
		frame = append(frame, field...)
		frame = append(frame, result...)
		frame = append(frame, '}')
	}
	_, err = w.Write(frame)
	return err
}

// readLength consumes the header block and returns its Content-Length.
func readLength(r *bufio.Reader) (int, error) {
	length := -1
	for first := true; ; first = false {
		line, err := r.ReadString('\n')
		if err != nil {
			if err == io.EOF && first && line == "" {
				return 0, io.EOF
			}
			return 0, fmt.Errorf("sdk: reading header: %w", unexpected(err))
		}
		line = strings.TrimRight(line, "\r\n")
		if line == "" {
			break
		}
		name, value, ok := strings.Cut(line, ":")
		if !ok {
			return 0, fmt.Errorf("sdk: malformed header %q", line)
		}
		if !strings.EqualFold(name, "Content-Length") {
			continue
		}
		n, err := strconv.Atoi(strings.TrimSpace(value))
		if err != nil || n < 0 {
			return 0, fmt.Errorf("sdk: malformed header %q", line)
		}
		length = n
	}
	if length < 0 {
		return 0, errors.New("sdk: missing Content-Length")
	}
	return length, nil
}

// unexpected turns an end of input inside a message into an error that
// callers cannot mistake for a clean end of input.
func unexpected(err error) error {
	if err == io.EOF {
		return io.ErrUnexpectedEOF
	}
	return err
}
