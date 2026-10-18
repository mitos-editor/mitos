# `dap`

Owns Debug Adapter Protocol communication, adapter lifecycle, transports, and request handling.

`RequestSender` is an owned, cloneable transport handle for asynchronous requests.
Sequence numbers are shared with its client. Dropping a pending wait releases its
response entry; late replies stay in the transport and are discarded. Editor
selection and result-publication policy belong to `view`.
