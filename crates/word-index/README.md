# `word-index`

Unicode tokenization, reference-counted words, incremental rope updates, and
lexical matching. Editor events, document IDs, debounce queues, worker lifetime,
and cancellation policy belong to callers. `changed_words` prepares cancellable
replacement windows from two snapshots; callers can scan before locking the shared
index, then apply the windows with `WordIndex::replace_words`. Benchmarks and multilingual fixtures
live beside the engine.
