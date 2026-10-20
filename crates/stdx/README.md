# `stdx`

Provides general-purpose utilities shared across crates, including filesystem, path, environment, range, rope, and URI helpers.

Editor positions count Unicode scalar values. The rope helpers convert those positions to Ropey 2 byte offsets explicitly; native Ropey slices and mutations use byte offsets. Line helpers use the editor’s selected LF or Unicode line convention.
