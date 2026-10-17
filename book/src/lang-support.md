# Language support

This table lists built-in support for each language. Language servers provide
completion, error messages, and other code features; install them separately
using the [setup guide](./language-server-setup.md).

In the server column, "first available" means Mitos chooses one server from
that list. Other listed servers can run alongside it.

Run `ms --health <language>` to check your installation, for example
`ms --health rust`. To change a language's settings, see
[Languages](./languages.md).

{{#include ./generated/lang-support.md}}
