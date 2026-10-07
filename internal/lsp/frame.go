package lsp

import (
	"bufio"
	"bytes"
	"fmt"
	"io"
	"strconv"
	"strings"
)

// WriteFrame writes one LSP JSON-RPC message with a Content-Length header.
func WriteFrame(w io.Writer, body []byte) error {
	var hdr bytes.Buffer
	hdr.WriteString("Content-Length: ")
	hdr.WriteString(strconv.Itoa(len(body)))
	hdr.WriteString("\r\n\r\n")
	if _, err := w.Write(hdr.Bytes()); err != nil {
		return err
	}
	_, err := w.Write(body)
	return err
}

// ReadFrame reads one LSP message body (JSON only, headers stripped).
func ReadFrame(r *bufio.Reader) ([]byte, error) {
	var contentLength = -1
	for {
		line, err := r.ReadString('\n')
		if err != nil {
			return nil, err
		}
		line = strings.TrimRight(line, "\r\n")
		if line == "" {
			break
		}
		lower := strings.ToLower(line)
		if strings.HasPrefix(lower, "content-length:") {
			n, err := strconv.Atoi(strings.TrimSpace(line[len("Content-Length:"):]))
			if err != nil {
				return nil, fmt.Errorf("bad Content-Length: %w", err)
			}
			contentLength = n
		}
	}
	if contentLength < 0 {
		return nil, fmt.Errorf("missing Content-Length header")
	}
	body := make([]byte, contentLength)
	if _, err := io.ReadFull(r, body); err != nil {
		return nil, err
	}
	return body, nil
}
