# `dap`

Owns Debug Adapter Protocol communication, adapter lifecycle, transports, and request handling.

`RequestSender` is an owned, cloneable transport handle for asynchronous requests.
Requests and reverse-request replies share sequence numbers allocated when queued,
preserving wire order across cloned senders. Dropping a pending wait releases its
response entry; late replies stay in the transport and are discarded. Editor
selection and result-publication policy belong to `view`.

Failed requests return `Error::Request` with the command, request sequence, raw
message, and original body. Display uses the structured error's format and
variables when available, then the short message, then a fallback. Missing or
malformed error details still remain request failures.

`Client` owns the transport lifetime. Dropping the client or incoming receiver
closes its IO tasks, even if request senders remain. EOF and IO failures reject
new requests, finish outstanding waits, and deliver one `ClientEvent::Closed`
notification so the editor can remove the session.

## Real-adapter smoke test

The `Real DAP adapter` CI job uses Go 1.27.2 and Delve v1.27.2 on Linux. It builds
an unoptimized Go fixture, launches it through the managed TCP transport, verifies
a source breakpoint, reads a stack frame and local variable, evaluates an
expression and a rejected expression, continues to a successful program exit,
and checks that disconnect closes the transport.

With these tools installed locally, run:

```sh
cargo test -p dap --test delve_smoke --locked -- --ignored --nocapture
```

`MITOS_DLV` can select a Delve executable outside `PATH`. The host must permit
debugging child processes. Missing tools or debugger permission errors fail the
explicit smoke run; ordinary workspace tests leave it ignored. The scenario uses
protocol events and bounded waits, without startup sleeps.
