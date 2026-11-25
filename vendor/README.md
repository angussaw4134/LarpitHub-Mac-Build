# Local authentication transport patch

`isideload` was copied from the installed crates.io `isideload 0.5.2` source. `src/auth/grandslam.rs` changes the Apple transport; `src/anisette/remote_v3/mod.rs` adds bounded connection/request timeouts for the helper. The Cargo patch is local to this installer; the registry cache and other loader projects are untouched.

Changes: force HTTP/1.1, disable idle connection pooling, bound connection/request waits, preserve TLS validation even if debug is requested, and check URL-bag HTTP status before parsing. No password replay, TLS bypass, proxy replacement, certificate/authentication bypass or Apple response rewriting is added.

The upstream temporary client-info override already present in iLoader is retained in the app's login setup. It is not presented as a new fix.
