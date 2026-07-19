# supernote-system

A Supernote Manta (A5X2) as the front end of a fully automated meeting
workflow, Google Workspace edition. Inspired by
[this r/Supernote post](https://www.reddit.com/r/Supernote/comments/1v0mpr5/).

- **Morning**: the templater reads Google Calendar and drops one 1920×2560
  template PNG per meeting into the Drive-synced `MyStyle/` folder — title,
  time, series, and area pre-printed, plus a strip of that meeting's still-open
  action items with tick-boxes.
- **In the meeting**: pick the template as the page background and write.
  Symbols mark delegation (`→ name`), deliverables owed (`◎ name` / `(o) name`),
  raise-with (`@ name`), priority (`!`), research requests (leading `?`), due
  dates. Ticking a printed item marks it done.
- **After**: the ingest agent picks up settled `.note` files, renders the ink,
  composites it over the template, and Claude transcribes it — it knows the
  zone layout, your people, and your areas.
- **Review**: the webapp shows pages awaiting review; fix the odd word, save.
  Saved actions feed the next templates; `@person` items appear on that
  person's next meeting sheet regardless of origin; confirmed `?` items kick
  off a deep-research run whose cited PDF report syncs back onto the device.

## Layout

| Path | What |
|---|---|
| `crates/core` | models, SQLite migrations, symbol grammar, template zone spec, routing rule |
| `crates/webapp` | axum server: ingest + transcription, review UI, dashboard, template data API, research worker |
| `crates/templater` | Calendar → template PNGs → `MyStyle/` |
| `crates/agent` | `.note` scan → render → dedup → composite → POST |
| `python/render_note.py` | thin `supernotelib` wrapper: `.note` → ink-as-alpha PNGs |
| `nix/` | packages + the `services.supernote` NixOS module |

## Deployment (NixOS)

Consume the flake and enable the module (see
`nix-config/systems/cerebro/supernote-system.nix`):

```nix
services.supernote = {
  enable = true;
  openFirewall = true;
  environmentFile = "/var/lib/supernote/secrets.env";
  rcloneConfigFile = "/var/lib/supernote/rclone.conf";
  googleClientSecretFile = "/var/lib/supernote/google-client-secret.json";
};
```

This runs four units: `supernote-gdrive` (rclone mount of the Drive tree),
`supernote-webapp` (port 8130), `supernote-templater` (06:30 daily), and
`supernote-agent` (every 15 min).

## One-time setup

1. **Google Cloud**: create a project, enable the **Calendar API**, create an
   **OAuth desktop client**, download the client secret JSON to
   `/var/lib/supernote/google-client-secret.json`.
2. **Consent run** (interactive, once): on the server, as the service user:
   ```sh
   sudo -u supernote env \
     GOOGLE_CLIENT_SECRET_FILE=/var/lib/supernote/google-client-secret.json \
     GOOGLE_TOKEN_CACHE=/var/lib/supernote/google-token.json \
     SUPERNOTE_MYSTYLE_DIR=/var/lib/supernote/gdrive/MyStyle \
     SUPERNOTE_FONT_DIR=$(nix build --print-out-paths nixpkgs#liberation_ttf)/share/fonts/truetype \
     supernote-templater
   ```
   Follow the printed URL, approve read-only calendar access; the refresh
   token is cached and every later run is headless.
3. **rclone**: `rclone config` a Drive remote scoped to the folder the
   Supernote syncs to; copy the resulting config to
   `/var/lib/supernote/rclone.conf` (owner `supernote`, mode 600).
4. **Anthropic key**: `/var/lib/supernote/secrets.env` containing
   `ANTHROPIC_API_KEY=sk-ant-...` (mode 600).
5. **On the Manta**: Settings → Cloud storage → Google Drive, sign in, sync.
   Use a dedicated daily notebook; per meeting page, choose the day's template
   as the page background (templates appear under My Styles after a sync).
6. **Seed people and areas** — this is what makes name/area resolution work:
   ```sh
   curl -X POST localhost:8130/api/areas -H 'content-type: application/json' \
     -d '{"name":"Infrastructure","aliases":"infra"}'
   curl -X POST localhost:8130/api/people -H 'content-type: application/json' \
     -d '{"name":"Alice Chen","aliases":"AC,alice","email":"alice@corp.example","area_id":1}'
   ```
   Emails must match calendar attendee addresses (that's how attendees and
   1:1 counterparts are detected).

## Handwriting grammar

```
→ name    (or -> name)   delegate to person
◎ name    (or (o) name)  deliverable owed to person
@ name                   raise at next meeting with person
!                        priority (repeat: !! > !)
? ...                    (leading) research request → deep-research agent
due: 2026-07-30, due 7/30  due date
D: / T: / N:             (leading) decision / takeaway / note
☐ / ☑                    printed carried-over tick-boxes
```

## Development

```sh
nix develop           # cargo, sqlx-cli, rclone, fonts, python
cargo test --workspace
cargo run -p supernote-webapp   # needs ANTHROPIC_API_KEY
nix build .#supernote-system .#supernote-renderer
```

The Python renderer can be exercised standalone against any `.note` file:
`supernote-render input.note outdir/` prints a JSON manifest and writes
per-page ink PNGs (alpha = ink).
