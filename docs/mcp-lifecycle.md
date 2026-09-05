# MCP cancellation and process ownership

Interrupting a turn cancels its pending MCP tool calls. Devo sends the MCP
`notifications/cancelled` notification with the original request ID when the
tool response future is dropped, including when its timeout expires. The MCP
server must honor this notification to stop remote work. Completed requests
are not cancelled, and the connection remains available for subsequent calls.

Local stdio servers belong to devo. On Windows they start suspended, enter a
kill-on-close job, and then resume, so descendants are included from startup.
Windows terminates that job's processes when devo exits, including abrupt
process termination that skips Rust destructors. Normal explicit shutdown also
terminates the local process tree. Unix retains its process-group cleanup.
