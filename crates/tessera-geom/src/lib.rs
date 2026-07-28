#![forbid(unsafe_code)]
// NOTE: this crate will eventually FFI into Clipper2 for polygon boolean/offset
// (plan §4.2). When that lands, narrow this to `#![deny(unsafe_code)]` and
// document each `unsafe` block's invariant at the FFI boundary (plan §12).
