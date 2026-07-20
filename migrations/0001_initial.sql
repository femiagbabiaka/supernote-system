-- Core schema for the Supernote meeting/action system.

CREATE TABLE areas (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    -- comma-separated shorthand forms ("eng", "infra") the transcriber may see
    aliases TEXT NOT NULL DEFAULT ''
);

CREATE TABLE people (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    -- comma-separated aliases: initials, nicknames, common misreadings
    aliases TEXT NOT NULL DEFAULT '',
    email TEXT,
    area_id INTEGER REFERENCES areas (id)
);

-- Standing meetings (1:1s, recurring syncs). Seeded via the API; each gets a
-- pre-printed template. There is no calendar integration by design: what the
-- system knows about meetings is what gets handwritten on the page.
CREATE TABLE meeting_series (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL UNIQUE,
    area_id INTEGER REFERENCES areas (id),
    is_one_on_one INTEGER NOT NULL DEFAULT 0,
    -- the counterpart for 1:1 series
    person_id INTEGER REFERENCES people (id),
    -- JSON array of regular-attendee people ids (used for action routing)
    attendee_ids TEXT NOT NULL DEFAULT '[]',
    -- template bookkeeping: generated PNG + the action ids printed on it,
    -- so ingest can rebuild the exact zone spec the page was drawn with
    template_path TEXT,
    carried_ids TEXT NOT NULL DEFAULT '[]'
);

-- One row per captured session. Created at ingest time from the page's
-- template + handwritten header (title / attendees), not from a calendar.
-- Reading/listening notes are sessions too (kind = 'reading').
CREATE TABLE meetings (
    id INTEGER PRIMARY KEY,
    -- idempotency key: "s<series_id>_<date>", "adhoc_<date>_<slug>",
    -- or "reading_<date>_<slug>"
    meeting_key TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL DEFAULT 'meeting' CHECK (kind IN ('meeting', 'reading')),
    series_id INTEGER REFERENCES meeting_series (id),
    title TEXT NOT NULL,
    area_id INTEGER REFERENCES areas (id),
    start_time TEXT NOT NULL, -- RFC 3339 (note capture time)
    -- JSON array of people ids: series regulars + handwritten attendees
    attendee_ids TEXT NOT NULL DEFAULT '[]',
    status TEXT NOT NULL DEFAULT 'captured'
        CHECK (status IN ('captured', 'transcribed', 'reviewed'))
);

CREATE INDEX meetings_start_time ON meetings (start_time);

CREATE TABLE actions (
    id INTEGER PRIMARY KEY,
    text TEXT NOT NULL,
    meeting_id INTEGER REFERENCES meetings (id),
    kind TEXT NOT NULL DEFAULT 'action'
        CHECK (kind IN ('action', 'decision', 'takeaway', 'note', 'research')),
    delegated_to INTEGER REFERENCES people (id), -- "→ name"
    owed_to INTEGER REFERENCES people (id),      -- "◎ name"
    raise_with INTEGER REFERENCES people (id),   -- "@ name" / "raise ... with name"
    priority INTEGER NOT NULL DEFAULT 0,         -- count of "!"
    due_date TEXT,                               -- ISO date
    status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'done')),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    closed_at TEXT
);

CREATE INDEX actions_status ON actions (status);

CREATE TABLE research_reports (
    id INTEGER PRIMARY KEY,
    action_id INTEGER NOT NULL UNIQUE REFERENCES actions (id),
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'running', 'ready', 'failed')),
    question TEXT NOT NULL,
    report_md TEXT,
    sources_json TEXT NOT NULL DEFAULT '[]',
    pdf_path TEXT,
    error TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    completed_at TEXT
);

-- Dedup ledger: which ink has already been ingested.
CREATE TABLE page_state (
    id INTEGER PRIMARY KEY,
    note_path TEXT NOT NULL,
    page_index INTEGER NOT NULL,
    ink_hash TEXT NOT NULL,
    ingested_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    UNIQUE (note_path, page_index)
);

CREATE TABLE transcriptions (
    id INTEGER PRIMARY KEY,
    -- nullable: a page that can't be matched to a meeting is still ingested
    -- and gets assigned during review
    meeting_id INTEGER REFERENCES meetings (id),
    page_image_path TEXT,
    raw_json TEXT NOT NULL,
    reviewed_json TEXT,
    status TEXT NOT NULL DEFAULT 'awaiting_review'
        CHECK (status IN ('awaiting_review', 'reviewed')),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX transcriptions_status ON transcriptions (status);
