# DesktopShepherd – instructions for Gemini CLI

The full rules for AI agents are in **`CLAUDE.md`** – they apply to Gemini unchanged. Read it,
then `README.md` and `docs/` (ARCHITECTURE, BUILDING, STATUS) before editing.

Summary of the non-negotiables:

- Stability first: no Explorer hooks/injection, no blocking calls into Explorer, no `unwrap`
  on runtime failures, never leave desktop icons hidden.
- Idle CPU ~0 %: repaint only when something changes.
- No network, no web engine. Keep the software-DirectX (WARP) fallback working; never add
  GL or eframe back.
- **Never run `cargo update`** (pinned crates keep the GNU toolchain build working).
- Stop the app with `scripts\stop.ps1` before `cargo build`.
- Keep `layout.json` backward compatible (`#[serde(default)]`, migrations in `app.rs`).
- The owner does not code: report results in plain words and say how you tested them.

Tip: with Gemini CLI, `@CLAUDE.md @docs/ARCHITECTURE.md` in your first prompt loads the
context; ask it to list the files it intends to change before editing.
