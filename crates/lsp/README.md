# `lsp`

Owns Language Server Protocol communication, server lifecycle, JSON-RPC transport, and request handling.

Client implementations are organized into private modules for process lifecycle,
workspace configuration, file operations, document synchronization, and feature
requests. Shared request dispatch remains on `Client`; transport tracking,
timeouts, and cancellation stay in the transport.
