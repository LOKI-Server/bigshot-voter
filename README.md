# Big-Shot Voting

A simple self-hosted voting website written in idiomatic Rust (edition 2024).

Admins open **voting panels** (images, audio, video, or other files). Approved members upload **one entry per panel** and cast **one vote per panel**. Votes can be changed until the panel is closed.

## Features

- **Ships as one executable.** CSS, JavaScript, and every HTML template are compiled into the binary — there is no `static/` folder to copy and nothing to go missing.
- **Markdown panel descriptions**, with an optional **related link**, editable after creation
- **First account is the sole overseeing admin** (auto-approved)
- Later users register with username + password and wait for **admin approval**
- **Account settings**: members change their username or password, and may add an optional **email to sign in with instead of a username**
- Only **approved** accounts can view panel content, upload, vote, and fetch media
- **Capsule tile grid** for entries: uniform previews regardless of source dimensions, a vote button under each one, and click-to-enlarge in a lightbox
- Secure password storage with **Argon2id**
- Signed **HTTP-only session cookies** + **CSRF** protection on forms
- **Range requests** on media, so long audio/video entries seek instantly
- Config lives in the **user config directory**; runtime data defaults to a **`data/` folder next to the server executable**

## Voting panel UI

Entries render as equally sized capsule tiles, so a 1600×1200 screenshot and a
400×900 portrait sit side by side without one dominating the page:

- The square preview crops to fill (`object-fit: cover`); nothing stretches.
- Previews scale with **nearest-neighbour** (`image-rendering: pixelated`), so pixel
  art and small icons stay crisp instead of turning into bilinear mush.
- **Click any preview to enlarge it** — a lightbox shows the media uncropped, with
  arrow-key/button navigation between entries and a link to the original file.
- The **vote button sits directly beneath each preview**, alongside the vote count
  and a share meter. Your own entry shows why it isn't votable; your current pick
  is marked and can be switched until the panel closes.
- The percentage is each entry's **share of every vote cast**, so the column adds up
  to 100%, and the meter bar is drawn to the same figure. (Shares are rounded
  independently so tied entries always show the same number, which can leave the
  total a point either side of 100.)
- Closed panels rank tiles and badge the winner.
- **Every row is a fixed height**, including the badge row, which renders even when
  empty. Tiles stay pixel-aligned with each other and nothing shifts when a
  "Your vote" badge appears after you vote.

Everything works without JavaScript except the lightbox, which is progressive
enhancement over plain links.

The site deliberately has **no icons or favicon** — text labels only. `/favicon.ico`
answers `204 No Content` so browsers stop asking.

## Panel descriptions

Descriptions are **Markdown**: bold/italic, `code`, links, lists, quotes, tables,
and footnotes. Rendering lives in [`src/markdown.rs`](src/markdown.rs), which is
what makes the output safe to embed unescaped:

- **raw HTML is dropped** — no tag an author writes reaches the page;
- link and image URLs are limited to `http`, `https`, `mailto`, and relative paths,
  so `javascript:` and `data:` payloads can't get through;
- headings are demoted two levels so a description can't introduce a competing `<h1>`.

List/card views show a flattened plain-text excerpt rather than raw Markdown syntax.

Admins can **edit a panel** at any time (title, description, related link) from the
admin dashboard or the "Edit panel" button on the panel itself. The **media type is
fixed after creation**, since existing entries were validated against it. The
optional **related link** must be an absolute `http(s)` URL and appears as a button
on the panel.

## Quick start

```bash
cargo run
```

Open <http://127.0.0.1:17843>, register the first account (you become admin), then create a panel from **Admin**.

### CLI options

| Flag / env | Default | Meaning |
|------------|---------|---------|
| `--bind` / `BIGSHOT_BIND` | `127.0.0.1:17843` | Listen address |
| `--data-dir` / `BIGSHOT_DATA_DIR` | from user-dir `bigshot.json`, else `{exe_dir}/data` | Database, secret, uploads (process override only) |
| `--set-data-dir` | — | Write `data_dir` into `bigshot.json` and exit (does not start the server) |
| `--clear-data-dir` | — | Clear the saved override (back to `{exe_dir}/data`) and exit |
| `--max-upload` / `BIGSHOT_MAX_UPLOAD` | 5242880 (5 MiB) | Max upload size in bytes |

```bash
cargo run -- --data-dir ./dev-data --bind 0.0.0.0:17843
```

### Configure the data directory (no server)

To point a permanent install at a custom folder without editing JSON by hand:

```bash
# Windows
bigshot-voter.exe --set-data-dir "F:\Servers\Big-Shot"

# Unix
./bigshot-voter --set-data-dir /var/lib/bigshot
```

That updates `bigshot.json` in the user config directory and exits. Relative paths
are stored absolute (resolved against the current working directory). Clearing:

```bash
bigshot-voter --clear-data-dir
```

**Changing the path does not move existing files** — stop the server, move the
folder yourself, then set the new path.

### Release build

```bash
cargo build --release
# Windows: target\release\bigshot-voter.exe
```

Copy that single file anywhere and run it — no other files are needed. On first
start it creates `bigshot.json` in the user config directory and `data/` beside
the executable (unless you override the data path).

For the smallest binary (opt-level `z` + fat LTO):

```bash
cargo build --profile release-size
# target/release-size/bigshot-voter[.exe]
```

Release profile uses thin LTO, stripped symbols, `panic = abort`, and `codegen-units = 1`. Linking uses `rust-lld` (see `.cargo/config.toml`).

### Windows installer

The app stores runtime data in `data/` next to the executable by default. A
classic install under `Program Files` is not writable for a normal user, so the
Windows package **defaults to a per-user, writable location**:

```text
%LOCALAPPDATA%\Programs\bigshot-voter\
```

That way `{exe_dir}\data` works without elevation or `--set-data-dir`. Config
still lives in `%APPDATA%\bigshot-voter\bigshot.json`. Uninstall removes
program files only; it does **not** delete `data\` or the AppData config.

Build a release binary, then compile the Inno Setup script (needs
[Inno Setup 6](https://jrsoftware.org/isinfo.php)):

```powershell
cargo build --release
iscc packaging\windows\bigshot-voter.iss
```

The installer lands in `target\windows-installer\` as
`BigShotVoting-<ver>-setup.exe`. Shortcuts and the post-install “Start now”
option run `bigshot-voter.exe` directly.

Sources: `packaging/windows/` (`bigshot-voter.iss`, `README-install.txt`).

## Data layout

On first start the server writes `bigshot.json` in the user config directory:

| Platform | Config path |
|----------|-------------|
| Windows | `%APPDATA%\bigshot-voter\bigshot.json` |
| macOS | `~/Library/Application Support/bigshot-voter/bigshot.json` |
| Linux | `$XDG_CONFIG_HOME/bigshot-voter/bigshot.json` or `~/.config/bigshot-voter/bigshot.json` |

```json
{
  "data_dir": null
}
```

Set `data_dir` to an absolute path (or a path relative to that config directory)
to store data somewhere else, either by hand or with `--set-data-dir`. Leave it
`null` (or run `--clear-data-dir`) for the default `data/` folder beside the
binary. **Changing `data_dir` does not move existing files** — stop the server,
move the folder yourself, then update the path.

If an older install still has `bigshot.json` next to the executable and the
user-dir file is missing, the server migrates it once on startup.

Priority for the data directory: `--data-dir` / `BIGSHOT_DATA_DIR` → `bigshot.json`
→ `{exe_dir}/data`.

Everything persistent lives under the data directory:

```
data/
  bigshot.db      # SQLite (users, panels, entries, votes)
  secret.key      # Cookie signing key (auto-generated)
  uploads/
    {panel_id}/
      {entry_id}.{ext}
```

Assets are served from content-hashed URLs (`/static/style.<hash>.css`) with a
one-year immutable cache, so an upgraded binary invalidates them automatically.

## How it works

1. **Empty database** → first `/register` creates the admin (approved).
2. Other people register → status `pending` until admin approves on `/admin`.
3. Admin creates a panel (media type: image / audio / video / other).
4. Approved users upload **one file** to that panel and vote for **one entry** (not their own).
5. Voters may **change** their vote while the panel is open; closing freezes votes. You cannot vote for your own entry.

## Account settings

`/settings` (linked from your name in the header) lets any signed-in member:

- **Change their username** — unique, 3–32 characters of letters/numbers/underscores.
- **Add an optional email**, then sign in with *either* the username or the email
  (matched case-insensitively). Clearing the field removes that sign-in route.
- **Change their password**.
- **Log out** — the button lives at the bottom of this page, not in the header.
  (Rejected accounts can't open settings, so they keep a header logout as a fallback.)

Both forms require the current password, since each one edits a sign-in credential.
No mail is ever sent, so the email is validated structurally rather than confirmed —
treat it as a convenience alias, not a verified address or a password-reset channel.

## Security notes

- Passwords are never stored in plaintext (Argon2id PHC hashes only).
- Sessions are HMAC-signed cookies; the key is `data/secret.key`.
- Uploads are extension/MIME allowlisted by panel type and stored under UUID names.
- Media is only served to **approved** logged-in users, with `Cache-Control: private`.
- Changing a password does **not** invalidate sessions on other devices; there is no
  session store to revoke against. Rotate `data/secret.key` to log everyone out.

## Development

```bash
cargo test
cargo clippy --all-targets
RUST_LOG=debug cargo run -- --data-dir ./dev-data
```

`static/` and `templates/` are **build inputs**, not runtime files: `src/assets.rs`
pulls them in with `include_bytes!` and Askama compiles the templates. Cargo tracks
both as dependencies, so editing a stylesheet, script, or template triggers a
rebuild on the next `cargo build` — but the running server won't pick up changes
until you restart it.

The schema migrates itself on startup, adding columns that older databases predate
(user emails, the panel related link).

## License

MIT
