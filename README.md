# Bigshot Voter for LOKI

Bigshot is a portable WASI component. LOKI hosts its HTTP interface and provides
identity, permissions, SQLite, streamed uploads/downloads, configuration, and
package lifecycle management. Bigshot keeps its voting UI, Markdown, media rules,
panels, entries, optional source files, and votes.

## Development prerequisites

Bigshot 0.6 needs LOKI 0.3 or later. It is built with `loki-sdk` 0.3, which uses
LOKI 0.3's typed host API; LOKI refuses packages built with an older SDK.

Check out LOKI as a sibling directory named `LOKI`. `Cargo.toml` points to
`../LOKI/crates/loki-sdk` with version `0.3.0`, so it builds before that SDK
release reaches crates.io. This dependency is the public SDK, with no use of
server implementation modules. Once 0.3.0 is published, the `path` can go.

From the LOKI checkout, install the tool and build the server:

```powershell
cargo install --path crates/cargo-loki --locked
cargo build -p loki-server
```

From this repository, run `cargo loki dev --server ../LOKI/target/debug/loki-server.exe`
(omit `.exe` on Linux/macOS). The tool creates a separate local installation,
prints development credentials, and watches code/templates/assets. Ctrl+C stops
it. Use `cargo loki check` or `cargo loki build` for shorter compile loops.

## Build and install

```powershell
rustup target add wasm32-wasip2
cargo loki package
```

Import `target/loki/bigshot-voter-0.6.0.loki-app` through LOKI's Applications
page and enable it. This is one package for all supported host operating systems
and CPU architectures, with no separately installed Wasmtime or app server.

Grant access to Members or selected people/groups. LOKI's owner receives voting
administrator access; other app administrators require an explicit `admin` grant.
Without a public hostname, use the local URL supplied by LOKI, such as
`http://bigshot-voter.localhost:7070`. A public hostname uses LOKI-managed Caddy.
The app has a separate browser origin while sharing LOKI's listener.

## What is shared

The plugin lives in `src/` and uses the `loki-sdk` Rust library. Its dependencies do not include Axum,
Tokio, SQLx, SQLite, a password hasher, or an OIDC HTTP client. It retains this
repository's templates, models, Markdown renderer, and media allowlists.
CSS, JavaScript and fonts are packaged as static assets, streamed by LOKI rather
than copied through guest memory.

LOKI supplies stable user IDs and current app roles on each request. Bigshot
stores its own records in the host-provided `app.db`; media stays under its
private uploads directory. The SDK supplies typed identity, database, form, upload, and response APIs;
there are no handwritten WIT bindings or JSON host calls in Bigshot. The SDK
encapsulates the versioned host interface.

The component defaults to a 32 MiB guest memory limit, 50 million execution fuel,
30 seconds of execution time, and four concurrent requests, additionally bounded
by LOKI's global component limit. Uploads default to 5 MiB per file and 16 MiB per
multipart request. Files stream in the host, including range downloads and
mandatory downloads for source files/active formats.

Each request uses a fresh Wasm instance; persistent state belongs in the app
SQLite database and uploads. Ordinary guest globals are not persistent state.

## Voting behavior

Approved members can submit one entry and one vote per panel. They can change
their vote while a panel is open and cannot vote for their own entry. Panel
closure freezes entries and voting; administrators can reopen a panel. Upload
allowlists depend on panel media type. A member can attach an allowed optional
master/source file to their own entry, including after the original upload.

Accounts are managed in LOKI. There are no separate Bigshot passwords or account
approval/recovery flows. Old standalone data and the former native app database
are not migrated into this fresh-install WASI edition.

## Verification

```powershell
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo clippy --target wasm32-wasip2 -- -D warnings
cargo test
```

Run the checks locally; this repository has no CI workflows. LOKI's independent
`cargo xtask wasi-smoke --loki PATH --package PACKAGE.loki-app` (run from LOKI's checkout) exercises
actual component installation, shared sign-in, panels, uploads, voting, role and
access revocation, failed-update rollback, and backup/restore with temporary data.

Bigshot is a plugin only: it no longer includes standalone server/account code,
a separate component crate, a manifest JSON file, or a Python packager. Cargo
metadata is the package source of truth; `cargo loki package` also records the
SDK version in the package, which LOKI checks before installing it.
