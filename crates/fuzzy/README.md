# `fuzzy`

Text, path, and completion matching policies backed by `nucleo-matcher`.
Owns reusable scoring and Unicode buffers plus highlight indices for asynchronous
picker queries.
Completion sessions, pickers, and word indexes own their matchers. Small-list
helpers cache a matcher per thread without a process-wide lock.

Editor state, provider ordering, matching jobs, and rendering belong to callers.
