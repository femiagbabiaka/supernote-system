# supernote-system

A Supernote Manta (A5X2) as the front end of a fully automated meeting
workflow. Inspired by
[this r/Supernote post](https://www.reddit.com/r/Supernote/comments/1v0mpr5/),
with one deliberate difference: **no calendar integration**. Meeting identity
comes from the page itself — the chosen template plus a handwritten header —
so no work-calendar credential exists and the only data that leaves the
machine is ink you chose to write.

- **Daily**: the templater renders one template PNG per standing meeting
  series — title pre-printed, that series' still-open action items as a
  tick-box strip — plus a generic ad-hoc template with blank `Meeting:` /
  `With:` lines and a reading/listening template with `Title:` / `By:` lines.
  They land in the Drive-synced `MyStyle/` folder.
- **In the meeting**: pick the series template (or the ad-hoc one and jot the
  title/attendees on the header lines) as the page background and write.
  Symbols mark delegation (`→ name`), deliverables owed (`◎ name` / `(o) name`),
  raise-with (`@ name`), priority (`!`), research requests (leading `?`), due
  dates. Ticking a printed item marks it done.
- **After**: the ingest agent picks up settled `.note` files, renders the ink,
  composites it over the template, and Claude transcribes it — including the
  handwritten header, resolved against your people directory. Meetings are
  created from what's on the page.
- **Review**: the webapp shows pages awaiting review; fix the odd word, save.
  Saved actions feed the next templates; `@person` items appear on that
  person's next meeting sheet regardless of origin; confirmed `?` items kick
  off a deep-research run whose cited PDF report syncs back onto the device.

## Layout

| Path | What |
|---|---|
| `crates/core` | models, SQLite migrations, symbol grammar, template zone spec, routing rule |
| `crates/webapp` | axum server: ingest + transcription, review UI, dashboard, template data API, research worker |
| `crates/templater` | series → template PNGs → `MyStyle/` |
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
};
```

This runs four units: `supernote-gdrive` (rclone mount of the Drive tree),
`supernote-webapp` (port 8130), `supernote-templater` (06:30 daily), and
`supernote-agent` (every 15 min).

## One-time setup

1. **rclone**: `rclone config` a Drive remote scoped to the folder the
   Supernote syncs to; copy the resulting config to
   `/var/lib/supernote/rclone.conf` (owner `supernote`, mode 600). A personal
   Google account is fine — the device syncs to whatever Drive you sign it
   into; nothing here touches a work account.
2. **Anthropic key**: `/var/lib/supernote/secrets.env` containing
   `ANTHROPIC_API_KEY=sk-ant-...` (mode 600).
3. **On the Manta**: Settings → Cloud storage → Google Drive, sign in, sync.
   Use a dedicated daily notebook (date-named notebooks, e.g. `20260720.note`,
   give meetings their correct date); per meeting page, choose the series
   template as the page background (templates appear under My Styles after a
   sync).
4. **Seed people, areas, and standing meetings** — this is what makes
   name/area resolution and routing work:
   ```sh
   curl -X POST localhost:8130/api/areas -H 'content-type: application/json' \
     -d '{"name":"Infrastructure","aliases":"infra"}'
   curl -X POST localhost:8130/api/people -H 'content-type: application/json' \
     -d '{"name":"Priya Natarajan","aliases":"PN,priya","area_id":1}'
   curl -X POST localhost:8130/api/series -H 'content-type: application/json' \
     -d '{"title":"1:1 Priya","is_one_on_one":true,"person_id":1,"area_id":1}'
   curl -X POST localhost:8130/api/series -H 'content-type: application/json' \
     -d '{"title":"Infra weekly","area_id":1,"attendee_ids":[1]}'
   ```
   Then run the templater once (or wait for the timer) and sync the device.

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

Header lines: `Meeting:` (pre-printed on series templates, handwritten on
ad-hoc pages) and `With:` (handwritten attendees; extra names on a series
page are merged in). The reading template uses `Title:` / `By:` instead —
notes there become `reading` sessions: takeaways and `?` research requests
work exactly as in meetings, and `@ name` still routes an item onto that
person's next meeting sheet ("discuss this book with...").

## Development

```sh
nix develop           # cargo, sqlx-cli, rclone, fonts, supernote-render
cargo test --workspace
cargo run -p supernote-webapp   # needs ANTHROPIC_API_KEY
nix build .#supernote-system .#supernote-renderer
```

The Python renderer can be exercised standalone against any `.note` file:
`supernote-render input.note outdir/` prints a JSON manifest and writes
per-page ink PNGs (alpha = ink).
