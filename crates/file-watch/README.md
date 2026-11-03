# file-watch

Native watch roots, ignore filters, readiness and coverage checks, and polling
of explicit metadata paths. Callers supply workspace classification and ordered
ignore-file paths; the crate does not discover editor configuration or resources.

Native callbacks publish event batches without blocking. Consumers validate each
batch against the watcher before applying it, including after asynchronous delivery.
Editor callbacks, reload decisions, LSP notifications, and UI subscriptions belong
to `view`.

Callers combine owner-scoped requests into a `Demand` and pass the union to
`Watcher::set_paths`. Removing the last demand for a recursive root releases it.
Root additions reuse the native watcher; removals rebuild it because the backend
has no unwatch operation. Metadata polling timestamps survive demand changes.
The editor tracks live-review document and LSP-registration ownership in `view`.
