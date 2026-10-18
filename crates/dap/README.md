# `dap`

Owns Debug Adapter Protocol communication, adapter lifecycle, transports, and request handling.

`RequestSender` is an owned, cloneable transport handle for asynchronous requests.
Sequence numbers are shared with its client. Dropping a pending wait releases its
response entry; late replies stay in the transport and are discarded. Editor
selection and result-publication policy belong to `view`.

`Client` owns the transport lifetime. Dropping the client or incoming receiver
closes its IO tasks, even if request senders remain. EOF and IO failures reject
new requests, finish outstanding waits, and deliver one `ClientEvent::Closed`
notification so the editor can remove the session.
