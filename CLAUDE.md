# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repository is

This is **ke (壳)**, a fork of [herdr](https://github.com/herdrdev/herdr) (Apache-2.0), currently based on upstream v0.9.1. herdr is a terminal agent runtime: a background server owns the PTYs, a TUI client draws them, and coding-agent CLIs run in its panes. ke adds a resident model ("管家") that lives in that workspace.

- **`FORK.md` is the authoritative fork record** — what ke is, every file modified relative to upstream with a one-line reason, and the upstream sync procedure. Read it before changing any file that also exists upstream, and append to its table when you change one.
- **`AGENTS.md` holds upstream's engineering rules.** Its *Universal Project Rules* (state/runtime separation, pure render, isolated platform code, decoupled detection, multiplicative performance paths, runtime/client boundary, stable endpoint contract) apply here. Its *Maintainer Workflow*, *Local Can Machine Workflow*, *Release Channels*, *Docs*, and *external contributor guardrail* sections describe the `herdrdev/herdr` repository and do **not** apply to this fork — ke has its own branch, tags, release workflow, and docs flow (below).
- The cargo package and bin target are still named `herdr` (upstream tests and tooling depend on `CARGO_BIN_EXE_herdr`); the rename to `ke` happens only at install/packaging time. Do not `cargo install` this repo — it would install a binary named `herdr`.
- Two version numbers: `Cargo.toml` `version` is the upstream base; `src/build_info.rs::KE_VERSION` is ke's own release version.

## Commands

```bash
cargo nextest run --locked                # full Rust suite
just test-one <filter>                    # one test or filter, e.g. just test-one composer_bar
just lint                                 # cargo fmt --check + clippy -D warnings
cargo build --release --locked            # build (build.rs compiles vendored libghostty-vt)
scripts/ke-install.sh [--no-build]        # install target/release/herdr as `ke` (default ~/.local/bin)
cargo run --release --locked -- --default-config   # print the default config template
```

- **Zig 0.16.0 is required to build** (pinned by `vendor/libghostty-vt/build.zig.zon`'s `minimum_zig_version`). Put it on `PATH` or set `ZIG`; `scripts/ke-install.sh` also falls back to `../ke-tools/zig-*/zig`.
- `just test` and `just check` run more than the Rust suite: Python maintenance tests, the UI hot-path architecture test, and **bun**-based suites (`scripts/release-workflows.test.ts`, `scripts/docs`, `src/integration/assets/*.test.ts`). `just check` additionally runs `just windows-lint`, which needs a one-time `just setup-windows-cross` (requires `xwin`). When bun or the Windows SDK is unavailable, run the parts that matter directly:

```bash
python3 -m unittest scripts.test_config_reference_check scripts.test_ui_hot_path_architecture
python3 scripts/config_reference_check.py     # config model vs. website config reference
```

- Working from inside a running ke/herdr session, clear the inherited environment so the debug binary talks to the debug `ke-dev` server instead of the installed one. Tests need `HERDR_ENV` cleared too, or they pick up the surrounding session:

```bash
env -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH cargo run -- <command>
env -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH -u HERDR_ENV cargo nextest run --locked
```

- `tests/cli/` only compiles on non-macOS unix (see the gate in `tests/cli.rs`), so CLI changes must be validated with a full run on Linux.

- Do **not** run `just install-hooks`: upstream's `commit-msg` hook only accepts conventional types (`feat`, `fix`, …) and would reject this fork's `ke:` commit subjects.

## Architecture

### Inherited from herdr

Server owns runtime state and PTYs; the TUI is a thin client over a private socket; agents and scripts drive the same runtime through the JSON API socket. `src/app/` is state (`state.rs`, pure data) / mutations (`actions.rs`) / API surface (`api.rs`, `api/`); `src/ui/` renders from `&AppState` without mutating; `src/detect/` matches manifest patterns against a pane's bottom-buffer snapshot; `src/platform/` holds all OS-specific behavior; `src/protocol/` and `src/api/schema/` are the wire contracts. `src/client/shell/` is the client-side TUI shell (sidebar, overlays, mouse, settings).

Because ke is a fork tracking upstream, **prefer new ke-only files hooked in with a single line** over edits spread through upstream files — that is what keeps merges cheap (`src/ke_env.rs` and the `mod composer; mod ke_panel;` lines placed at the *end* of `src/client/shell.rs`'s module list are the pattern to copy).

### The ke layer

- `src/ke/` — everything ke adds that is not TUI chrome:
  - `paths.rs` — per-session `resident/` directory next to the session's socket (`composer.sock`, `panel.json`, `chat.jsonl`, `taskboard.json`, `resident.log`).
  - `resident/` — the `ke resident` process: one per session, started and restarted by `supervisor.rs` from the server when `[ke.model].enabled` is set. It reads session state **through the normal JSON API** (socket handed to it in `HERDR_SOCKET_PATH`), never server internals, and exits when the API stops answering.
  - `resident/sessions.rs` — the cross-session roster. A resident serves one session but polls **every** running session (`session::list_sessions()`), because the pane waiting for you is often in another one. Two rules: `pane_id` is unique only within a session, so pane identity is keyed on `(session, pane_id)` (`AgentKey`); and only the **local** session's failures count toward shutdown — a remote session stopping must cost its own rows and nothing else. Pane *contents* stay local-only, so a project marked as never leaving the machine cannot be routed around through another session's model.
  - `redact.rs` — rule-based redaction gate applied to composer text and model context.
  - `chat_log.rs` — append-only `@ke` transcript shared by resident (writer) and client (tail reader).
  - `slash.rs` — `/ke …` commands and `//` passthrough, parsed client-side.
- `src/client/shell/` ke modules — `composer.rs` (the input dock: right-hand column on desktop, bottom bar when narrow), `ke_panel.rs` (sidebar panel read from `panel.json` by mtime), `ke_chat.rs` (`@ke` transcript overlay), `ke_cmd.rs` (`/ke` execution), `ke_model.rs` (model settings page).
- Flow: composer submit → local socket → resident processor (redact / `done` when `@ke` is handed to the model) → `agent.prompt` or `pane.send_input`. The resident polls `agent.list` and writes `panel.json`/`chat.jsonl`; the client polls those files by mtime. **This adds no wire-protocol changes** — keep it that way.
- Isolation from an installed upstream herdr: app dir is `ke` (release) / `ke-dev` (debug) in `src/config/io.rs`, self-update is refused (`KE_DISABLE_SELF_UPDATE`), `integration install/uninstall` is disabled (ke reuses the hooks upstream herdr installed for each CLI), and panes get `KE_ENV=1` plus `HERDR_*` pointing at ke's own socket. `src/ke_env.rs` strips inherited `HERDR_*` when ke starts inside an upstream herdr pane.

## Working in this fork

- **Register every change to an upstream file**: add a `// Modified by ke: <why>` comment at the change site and a `日期 | 文件 | 修改` row in `FORK.md`. New ke-only files start with `//! Added by ke: …`. This is an Apache-2.0 §4 obligation, not bookkeeping.
- **Syncing upstream**: `git fetch upstream --tags && git merge upstream/<tag>` on `ke/main`; prefer upstream behavior in conflicts, then re-verify each `FORK.md` row still holds. The `upstream` push URL is deliberately `DISABLED`. New upstream tests that hardcode the `herdr-dev` app dir must be renamed to `ke-dev`.
- **Config keys**: any new `[ke]`/`[ke.model]`/`[ke.redact]` key must be added to `docs/next/website/src/data/config-reference.json` (and to the commented `[ke]` block in the default template in `src/main.rs` when user-facing), or `scripts/config_reference_check.py` fails.
- **New client→server methods** must be allowed in `src/server/client_commands.rs` and appended to `tests/fixtures/endpoint-method-shapes-v1.json`. Append only — never edit an existing entry.
- **Branch and tags**: work on `ke/main`; ke releases are `ke-v*` tags. Never push `master` or `v*` tags to this repo — upstream's `release.yml`, `preview.yml`, and `distribution.yml` trigger on those.
- Upstream's release, preview, changelog (`docs/next/CHANGELOG.md`), `distribution/latest.json`, and `skills/herdr/SKILL.md` flows are not ke's. ke's user docs are `docs/next/website/src/content/docs/ke.mdx` (+ `zh-cn/`, `ja/`).

## Commit and release

- Commit subjects in this fork use the `ke: ` prefix (`ke: release ke-v0.3.3`, `ke: 管家对话、斜杠命令与 Windows 支持`). Lowercase, no emojis, no AI co-author lines. Propose the message and get alignment before committing.
- **Never `git add -A`.** The working tree usually carries ~150 unrelated rustfmt-dirty files left by a stray reformat; `git status` being loud is normal. Stage your own files by name, and `git diff` anything unexpected to confirm it is not pure formatting before it goes in. Do not push unless asked.
- **Code changes alone do not reach the user.** They test ke on another machine through `curl …/ke-install.sh` or `ke update`, so anything user-visible only lands once a `ke-v*` GitHub Release is published.
- Release: bump `KE_VERSION` in `src/build_info.rs`, commit, then

```bash
git tag ke-v<KE_VERSION> && git push origin ke/main ke-v<KE_VERSION>
```

`.github/workflows/ke-release.yml` verifies the tag matches `KE_VERSION` and publishes five artifacts (linux/macos × x86_64/aarch64, plus `ke-windows-x86_64.zip` containing `ke.exe` and its ConPTY runtime) with `ke-install.sh`, `ke-install.ps1`, and `SHA256SUMS`.

## Code conventions

No `unwrap()` in production code; `tracing` for logging; `#[allow]` only with a comment saying why. Platform-specific code is `#[cfg]`-gated with OS APIs living in `src/platform/` (`cfg!(...)` only for policy constants that compile on every target). Don't add dependencies without checking the existing ones first. Unit tests live beside the code in `#[cfg(test)] mod tests`; new `AppState`/`Workspace` behavior should be testable via `AppState::test_new()` / `Workspace::test_new()` without PTYs. ke's TUI behavior is covered in `src/client/shell/tests/` (`composer_bar.rs`, `ke_chat.rs`).
