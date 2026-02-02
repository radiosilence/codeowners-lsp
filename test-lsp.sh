#!/bin/bash
# Test script to interact with the LSP via JSON-RPC

LSP_BIN="./target/release/codeowners-lsp"
WORKSPACE="/Users/james.cleveland/workspace/radiosilence/codeowners-lsp/test-workspace"
CODEOWNERS_URI="file://${WORKSPACE}/.github/CODEOWNERS"

# Helper to create JSON-RPC message with Content-Length header
send_rpc() {
    local json="$1"
    local len=${#json}
    printf "Content-Length: %d\r\n\r\n%s" "$len" "$json"
}

# Create a temp file for responses
RESPONSE_FILE=$(mktemp)

echo "=== Test CODEOWNERS File ==="
cat "$WORKSPACE/.github/CODEOWNERS"
echo ""
echo "=== Running LSP ==="

# Start LSP in background, capture output
(
    # Initialize
    send_rpc '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"processId":null,"rootUri":"file://'"$WORKSPACE"'","capabilities":{"textDocument":{"publishDiagnostics":{"relatedInformation":true}}},"initializationOptions":{"path":".github/CODEOWNERS"}}}'

    sleep 0.3

    # Initialized notification
    send_rpc '{"jsonrpc":"2.0","method":"initialized","params":{}}'

    sleep 0.3

    # Open the CODEOWNERS file (this should trigger diagnostics)
    CONTENT=$(cat "$WORKSPACE/.github/CODEOWNERS" | jq -Rs .)
    send_rpc '{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"'"$CODEOWNERS_URI"'","languageId":"codeowners","version":1,"text":'"$CONTENT"'}}}'

    sleep 1.5

    # Request inlay hints
    send_rpc '{"jsonrpc":"2.0","id":4,"method":"textDocument/inlayHint","params":{"textDocument":{"uri":"'"$CODEOWNERS_URI"'"},"range":{"start":{"line":0,"character":0},"end":{"line":40,"character":0}}}}'

    sleep 0.5

    # Exit
    send_rpc '{"jsonrpc":"2.0","method":"exit","params":null}'

) | timeout 8 $LSP_BIN 2>/dev/null > "$RESPONSE_FILE"

echo ""
echo "=== Diagnostics Found ==="
# Extract diagnostics and format nicely
cat "$RESPONSE_FILE" | tr '\r' '\n' | grep -o '{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics".*' | head -1 | jq -r '.params.diagnostics[] | "Line \(.range.start.line + 1): [\(.code)] \(.message)"' 2>/dev/null || echo "No diagnostics parsed"

echo ""
echo "=== Inlay Hints (file match counts) ==="
cat "$RESPONSE_FILE" | tr '\r' '\n' | grep -o '{"jsonrpc":"2.0","result":\[.*\],"id":4}' | jq -r '.result[] | "Line \(.position.line + 1):\(.label)"' 2>/dev/null || echo "No inlay hints parsed"

rm -f "$RESPONSE_FILE"
