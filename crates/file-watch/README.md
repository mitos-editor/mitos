# file-watch

Native watch roots, ignore filters, readiness and coverage checks, and polling
of explicit metadata paths. Callers supply workspace classification and ordered
ignore-file paths; the crate does not discover editor configuration or resources.

Native callbacks publish event batches without blocking. Consumers validate each
batch against the watcher before applying it, including after asynchronous delivery.
Editor callbacks, reload decisions, LSP notifications, and UI subscriptions belong
to `view`.
