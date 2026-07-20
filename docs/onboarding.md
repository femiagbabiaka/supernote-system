# Onboarding guide

Zero to working system: a Supernote Manta whose meeting and reading notes are
automatically transcribed, turned into tracked action items, routed onto the
right person's next agenda, and — for `?` items — researched by an agent whose
cited PDF report syncs back onto the device.

**What you need before starting:**

- A Supernote Manta (A5X2) — other models work but templates are sized 1920×2560.
- A **personal** Google account for Drive sync. Nothing in this system touches
  a work account; it exists purely as the pipe between the device and your
  server.
- A NixOS host that's always on (this guide assumes `cerebro`, with the
  flake input already wired in `nix-config`).
- An Anthropic API key (console.anthropic.com).

Time: ~30 minutes of setup, plus one meeting to try it on.

---

## 1. Deploy the service

The module is already wired into nix-config (`systems/cerebro/supernote-system.nix`).

```sh
cd ~/src/nix-config
sudo nixos-rebuild switch --flake .#cerebro
```

This creates the `supernote` user, `/var/lib/supernote/`, and four units:

| Unit | What | When |
|---|---|---|
| `supernote-gdrive` | rclone mount of the Drive tree | always on |
| `supernote-webapp` | transcription, review UI, dashboard (port 8130) | always on |
| `supernote-templater` | regenerates template PNGs | daily 06:30 |
| `supernote-agent` | ingests new handwriting | every 15 min |

**Expected state right now:** `supernote-gdrive` fails (no rclone config yet)
and drags the rest with it. That's fine — secrets come next.

```sh
systemctl status supernote-gdrive supernote-webapp
```

A note on exposure: the webapp has **no authentication**. `openFirewall = true`
makes it reachable on the LAN — keep it on a trusted network (or tailnet), or
set `openFirewall = false` and use SSH forwarding.

## 2. Secrets

Both files live under `/var/lib/supernote/`, owned by `supernote`, mode 600,
never in the Nix store.

**Anthropic key:**

```sh
sudo tee /var/lib/supernote/secrets.env >/dev/null <<'EOF'
ANTHROPIC_API_KEY=sk-ant-...
EOF
sudo chown supernote:supernote /var/lib/supernote/secrets.env
sudo chmod 600 /var/lib/supernote/secrets.env
```

**rclone remote** (cerebro is headless, so the OAuth hop happens on your laptop):

```sh
# On cerebro:
rclone config
#  n) New remote  →  name: gdrive  →  storage: drive
#  scope: drive (full access — it needs to write templates and PDFs)
#  Edit advanced config: No
#  Use auto config?  →  n   (headless)
# It prints a `rclone authorize "drive" ...` command.

# On your laptop (with a browser):
rclone authorize "drive"
# Sign in with the PERSONAL Google account, paste the token back on cerebro.

# Then move the config into place:
sudo mkdir -p /var/lib/supernote
sudo cp ~/.config/rclone/rclone.conf /var/lib/supernote/rclone.conf
sudo chown supernote:supernote /var/lib/supernote/rclone.conf
sudo chmod 600 /var/lib/supernote/rclone.conf
```

Don't restart anything yet — the mount points at `gdrive:Supernote`, and that
folder doesn't exist until the device's first sync.

## 3. Device setup

On the Manta:

1. **Settings → My account / Cloud & sync → Google Drive**, sign in with the
   personal account, and run a first sync.
2. Check Drive in a browser: the device creates its sync tree (folders like
   `Note/`, `Document/`, `MyStyle/`). Note the **root folder name** — if it
   isn't `Supernote`, set `services.supernote.driveRemote = "gdrive:<name>"`
   in nix-config and rebuild.
3. Create a **daily notebook named by date**, e.g. `20260721`, in the synced
   `Note/` folder. The date in the filename is how captures get their meeting
   date; one notebook per day, a fresh page per meeting.

Now start the pipeline and confirm the mount:

```sh
sudo systemctl restart supernote-gdrive supernote-webapp
systemctl status supernote-gdrive          # should be active (running)
sudo -u supernote ls /var/lib/supernote/gdrive/   # Note/ Document/ MyStyle/ ...
```

## 4. Seed your world

This is what makes handwriting resolve to real people and routing work.

```sh
sudo cp /path/to/seed.example.toml /var/lib/supernote/seed.toml
sudo $EDITOR /var/lib/supernote/seed.toml
supernote-seed /var/lib/supernote/seed.toml
```

Guidelines:

- **People aliases**: initials, nicknames, and plausible misreadings of your
  handwriting — the transcriber resolves against these, so be generous.
- **1:1 series** (`one_on_one = "Name"`): the template pre-fills `With:`, and
  `@Name` items from anywhere land on its sheet.
- **Group series** (`attendees = [...]`): regulars drive routing the same way.

Re-running the seed is safe (upserts by name/title) — keep the file and
re-apply when your org or cadence changes.

Generate templates now instead of waiting for 06:30, then sync the device:

```sh
sudo systemctl start supernote-templater
sudo -u supernote ls /var/lib/supernote/gdrive/MyStyle/
# s1_1-1-priya.png  s2_infra-weekly.png  adhoc.png  reading.png
```

Sync on the Manta, then confirm the templates appear under
**My Styles** when changing a page's background. **This is the one step with
an external unknown** — if templates don't show up, check whether the device
expects a different MyStyle path and adjust `mystyleSubdir`.

## 5. Your first meeting

1. New page in today's daily notebook → set page background → pick the
   meeting's series template (or `adhoc` and write the title/attendees on the
   header lines).
2. Write normally. Mark items with the grammar:

   ```
   →  name      (or -> name)    delegate to person
   ◎  name      (or (o) name)   deliverable owed to person
   @  name                      raise at next meeting with person
   !                            priority (!! higher)
   ?  ...                       (leading) research request
   due: 2026-07-30  or  due 7/30
   D: / T: / N:                 (leading) decision / takeaway / note
   ```

   A realistic page:

   ```
   Meeting: [1:1 Priya — pre-printed]
   With:    ___________  (add extra attendees here if any)

   ☐ Draft capacity plan ... #42      ← tick when done
   ─────────────────────────────
   quota bump approved, D: go with reserved instances
   send updated budget → Bob due 7/30
   ? survey of GPU sharing schedulers !
   review offsite agenda @ Dana
   ```

3. After the meeting: nothing. The device syncs on its own; the agent waits
   until the notebook has been **untouched for 2 hours** (so it never ingests
   mid-edit), then picks it up on its 15-minute timer. Expect notes to appear
   for review ~2–2.5 hours after you stop writing. To force it sooner:

   ```sh
   sudo systemctl start supernote-agent
   ```

## 6. Review

Open `http://cerebro:8130/review`. Each transcribed page shows the page image
next to editable parsed items — fix the odd word, adjust a person/kind/due
date, drop noise rows, assign a meeting if the page didn't match one. **Save**
finalizes: actions are created (deduped against existing open ones), ticked
carried-over items close their originals, and confirmed `?` items enqueue
research runs.

`http://cerebro:8130/dashboard` is the open-actions view: priority, due dates,
who owes what, research status.

The loop closes at the next templater run: tomorrow's templates carry today's
still-open items, each with a tick-box and printed `#id`.

## 7. Reading & listening notes

Same flow, different template: pick `reading`, write the work on `Title:` and
the author/creator on `By:`. Captures become `reading` sessions rather than
meetings. Takeaways (`T:`), research requests (`?`), and person routing
(`@ name` → "discuss this with...") all work identically.

## 8. Research runs

A confirmed `?` item kicks off a pipeline on the server: question refinement →
web search → cited Markdown report → PDF written to
`Document/Research/<date>_<slug>.pdf` in the Drive tree, which syncs onto the
device. Status shows on the dashboard (`pending`/`running`/`ready`/`failed`
with a retry button). Cost guard: at most `researchDailyCap` runs per day
(default 5) — raise it in nix-config if you're research-happy.

## 9. Troubleshooting

| Symptom | Look at | Likely fix |
|---|---|---|
| Nothing ingests | `journalctl -u supernote-agent -e` | Mount down? Notebook touched <2h ago? Page genuinely blank? |
| Mount keeps failing | `journalctl -u supernote-gdrive -e` | Token expired → redo `rclone authorize`; wrong folder → `driveRemote` |
| Page shows "(unassigned)" in review | — | Page background wasn't a series template; assign the meeting in the review form |
| Names come out unresolved (warning in review) | — | Add aliases to `seed.toml`, re-run `supernote-seed` |
| Transcription errors | `journalctl -u supernote-webapp -e` | API key wrong/expired; check `secrets.env` |
| Templates don't reach the device | `sudo -u supernote ls .../MyStyle/` then Drive in a browser | rclone upload vs device-side sync — find which hop dropped it |
| Edited an old page | nothing needed | The ink hash changes, so the page re-ingests as a new transcription |

Manual runs of anything: `sudo systemctl start supernote-templater` /
`supernote-agent`. The database is `/var/lib/supernote/supernote.sqlite3` if
you ever want to poke at it directly (`sqlite3` is in the dev shell).

## 10. Tuning knobs (nix-config)

| Option | Default | Notes |
|---|---|---|
| `templaterOnCalendar` | `*-*-* 06:30:00` | when templates regenerate |
| `agentInterval` | `15min` | ingest scan cadence |
| `researchDailyCap` | `5` | max research runs/day |
| `transcribeModel` / `researchModel` | `claude-opus-4-8` | per-call model choice |
| `port` / `openFirewall` | `8130` / your call | webapp exposure |
| `fontPackage` / `fontName` | Liberation Sans | template + PDF typography |

Costs, roughly: transcription is one vision call per page (cents); research
runs are the expensive path (tens of cents to a couple of dollars each,
capped daily); everything else is free.
