*storage.txt*   A packed store for the conversation graph — measured, then designed

                     SHANNON CODE — THE PACKED STORE

Steven, 2026-10-01: *"with SQLite can we do the correct compression? Make sure
to think about this carefully and consider our graphs. … a later feature will
also be encryption … we don't need to build that now."*

STATUS: PROPOSED (2026-10-01). Nothing here is built. The numbers come from
`tools/probe/storeproto` run on Steven's `~/.claude/projects`; rerun it on a
bigger history before deciding (`cargo run --release -- --levels` from that
directory; it prints sizes, never content). Extends design.md §1.3.1/§1.5,
whose [OPEN] "when to flip the default backend to SQLite" this answers.

The short answer: **yes — because compression belongs to the ROW, and the
graph already says what a row is.** SQLite does not compress; we compress each
turn's body ourselves before it is stored, and the content-addressed graph
makes that the only unit that is both cheap to read and safe to share.

==============================================================================
1. WHAT WAS MEASURED                                       *storage-measured*

Corpus: 8 Claude Code sessions, 311.9 MiB of JSONL, regrouped into 1256
Shannon-shaped turns (a prompt through every tool round it caused): 155.6 MiB
of message bodies. Turns are large — p50 38 KB, p90 337 KB, max 3.7 MiB.
Warm OS cache, one machine, Claude Code's content rather than Shannon's own.

>
    WHERE THE BYTES ARE                    share   zstd-3, alone
    ──────────────────────────────────────────────────────────────
    JSON scaffolding (keys, ids, escaping)   27%   → almost nothing
    thinking signatures                      26%   1.35×   ciphertext
    tool output text                         14%   3.09×
    tool-call inputs                         14%   3.63×
    images (base64 PNG)                      13%   1.66×
    prompt + reply text                       6%   2.30×
    thinking text                            <1%

    ENCODING (whole corpus, zstd-3)          ratio   random access
    ──────────────────────────────────────────────────────────────
    one frame per turn                       2.50×   one frame
      + trained dictionary (110 KiB)         2.57×   frame + dict
    delta against the previous turn          2.50×   a chain
    whole session in one frame (bound)       2.69×   none

    LEVEL     1: 2.38×  750 MiB/s    3: 2.50×  530 MiB/s
              9: 2.58×  125 MiB/s   19: 2.62×    8 MiB/s  (decode ~1.3 GiB/s)

    SQLITE (WAL, 1256 turns)   structure only   one body   1040-turn session
    bodies inline                  1.1 ms        105 µs        102 ms
    bodies in their own table      0.3 ms        106 µs        103 ms
<
Two facts carry the design. **Per-turn frames lose only 7% against the best
case** (2.50× vs 2.69×), and **delta chains along a session gain nothing** —
turns do not repeat each other; where content does repeat, the graph already
shares it. And the ratio is capped by content, not codec: a third of the bytes
is ciphertext or already-compressed images, which is why level 19 buys 5%.

Projection for this corpus: 311.9 MiB → about 58 MiB, ~5×. Roughly 2× is not
storing Claude Code's per-record envelope, 6% is dedup (281 identical turn
bodies, mostly resumed sessions that copy history, some repeated screenshots),
and 2.5× is zstd. Robust to Shannon's own scaffolding being smaller: the
scaffolding compresses to ~3 MiB either way.

==============================================================================
2. THE GRAPH DECIDES THE UNIT                                  *storage-unit*

**Identity sits above encoding.** `TurnId` is BLAKE3 over the turn's plaintext
messages and config (`graph/turn.rs`, `TurnId::compute`). The codec is a column
on the row, not part of the id — git does the same, hashing objects before
zlib. So recompressing, changing a dictionary, or encrypting later never
changes an id, never breaks a `Slice`, and never invalidates a reference a
user typed (`:open <turn-id>`).

**The unit of compression is the unit of sharing.** The graph shares at turn
granularity: an episode is a piece table of `Own` turn ids and `Slice`s of
other episodes. Any other unit fights it:

>
    bigger (session packs)   +7% at best, and a shared turn's bytes now live
                             inside ANOTHER episode's pack: `:delete`,
                             `rewrite_episode` and GC must repack
    smaller (per message)    nothing references a message alone; small frames
                             compress worse (1.86× under 4 KiB)
    per turn                 every graph operation — branch, squash, re-root,
                             delete — is metadata; compressed bytes are written
                             once, at seal, and never rewritten
<
**Where a delta WOULD pay: edits.** An edit re-seals a turn with one message
changed — a near-copy of a turn the graph names. Compressing it with its
source as the dictionary should cost little more than the change (zstd's
patch-from shape), with depth one (always against a full frame, so a read is
at most two decodes). Not measured — Claude Code has no edits to measure — and
it makes the base a GC dependency (a base cannot be dropped while a delta
needs it). Deferred until Shannon's own store shows large turns being edited.

==============================================================================
3. THE SCHEMA                                                *storage-schema*

>
    turn       (id PK, model, size, codec)                  content header
    turn_body  (id PK → turn, frame BLOB)                   compressed — lazy
    episode    (id PK, config, origin)
    segment    (episode, seq, own: turn id + its provenance   the piece table;
                | slice: source, start, end)                  occurrences — dense
    ref        (name PK, episode)
    blob       (hash PK, media type, codec, bytes)          later: images
<
- **Bodies in their own table.** A structure scan never touches a body page:
  0.3 ms vs 1.1 ms warm, and the gap should widen cold, since inline rows
  spread structure across pages full of blob (inferred, not measured).
- **Dedup is `INSERT OR IGNORE` on the content id** — what JSONL could not do
  without a scan.
- **Writes become transactions.** `append_turn` is one; `rewrite_episode`'s
  "must be atomic" (`store/mod.rs`) is one, where JSONL writes a replacement
  and renames. WAL lets many agents write one project store at once — the
  reason SQLite, not compression, is the default this proposes.
- **GC is reachability**: a turn lives while some live episode's derived
  sequence includes it — a recursive query over segments and the episodes
  their slices name.
- **Provenance lives on the occurrence** — the `Own` entry that produced the
  turn — and content on the id (BUILT 2026-10-02, design.md §1.1). Found while
  designing this: keyed by `TurnId`, two independently sealed identical turns
  shared one provenance, last loaded wins. The split is also what the lazy
  load wants: occurrences (time, usage, stop reason) are small structure,
  loaded eagerly; messages are the content, loaded on demand.
- **An occurrence's `handles` keeps `NULL` apart from `[]`** — the secret
  handles the turn recorded holding (secrets.md Q4). `[]` is "held none"
  (every new turn's default, fail-closed); `NULL` is "written before the field
  existed", the only state that falls back to scanning the turn's own text.
  A schema that stored both as empty would put every new turn on that scan;
  the store contract (`identical_turns_keep_their_own_provenance`) checks it.

==============================================================================
4. LOAD LAZILY — THE WIN THAT IS NOT COMPRESSION                *storage-lazy*

`Store::load_graph` reads and parses every turn of every episode at launch,
and `:open` loads the session's turns again. Parsing this corpus as JSONL took
1.0 s, and that grows with every month of history. From SQLite, the structure
alone is 0.3 ms, and a body is ~100 µs including decompression of an average
124 KB turn.

So the graph must hold turns without their messages: structure eagerly, bodies
on demand, decoded once per view and cached — never per frame. **This is the
largest code change in the plan, and it is needed whatever we decide about
compression**: a JSONL store that loaded lazily would need an offset index,
which is what SQLite is.

BUILT (2026-10-02) — the shape, so a later backend can change under it:

- **The seam is two trait methods.** `Store::load_structure` (episodes,
  occurrences, and per turn its model and an OUTLINE — role, first line with
  text, length per message) and `Store::load_bodies(ids)`. Both default to a
  full load, so JSONL is unchanged and correct; a backend that can load less
  overrides the pair. Nothing above the trait knows which kind it holds.
- **A body is `Option` in the graph.** `Graph::messages` says `None` for an
  unloaded body, and `Graph::records` refuses a sequence with one rather than
  return it shorter — a session must never read as having fewer turns than
  are on disk.
- **One door to the words: `store::hydrate(store, graph, episodes)`**, which
  loads exactly the bodies those episodes' derived sequences lack. The app
  reaches it through `App::graph_reading` (and `squash_graph` for marks); every
  path that reads turns — open, clone, checkout, edit, return, squash, delete,
  `:continue`'s tip, compose's adoption — goes through it.
- **Views read outlines only.** `:graph` rows and the turn inspector draw from
  structure; the message inspector, the one view that prints words, loads its
  turn when selected (`App::load_inspected_body`).
- **The suite runs lazy everywhere.** `MemoryStore` is lazy itself, and every
  store-backed app test wraps its store in `LazyBodies`, which strips bodies
  from any `load_structure` — so the JSONL tests meet unloaded bodies the way
  production on SQLite will.

==============================================================================
5. WHAT DOES NOT COMPRESS                           *storage-incompressible*

- **Thinking signatures, 26%.** Measured: 40.7 MiB of signatures beside
  0.3 MiB of thinking text. My reading (recalled from the API docs, not
  verified here) is that the signature carries the reasoning encrypted, so it
  is ciphertext in base64 — 1.35× is base64's own overhead coming back, and no
  codec does better. Whether old signatures must be kept at all is an API
  question, not a storage one: a known unknown.
- **Images, 13%.** A `blob` table keyed by hash: stored decoded (−25% at
  once), deduplicated (this corpus repeats four screenshot turns), loaded only
  when sent — the chat shows a placeholder. Lands with image support or the
  importer (TODO 38).
- **A trained dictionary**: +3% overall, but 1.86× → 3.20× on turns under
  4 KiB — 0.1 MiB here. Deferred; the `codec` column distinguishes plain zstd
  from zstd-with-dictionary-N, and adding it later changes no id.
- **Level 3.** Level 19 is +5% at 1/65 the speed: no background recompression.

==============================================================================
6. CLEAR TEXT, PACKED, AND MEMORIES                           *storage-modes*

Steven asked for both shapes, chosen by a setting. They are two `Store`
backends, which the trait was built for:

>
    clear text   JsonlStore (today)   `cat`, `grep`, `tail -f`; one reader
    packed       SqliteStore          lazy, transactional, many writers, ~5×
<
Converting is a load from one and a write to the other through the trait
(`shannon store convert`, later). The contract tests that already hold
`MemoryStore` and `JsonlStore` to one behaviour hold the new one too.

**Memories stay plain text files, always.** Every memory directory under
`~/.claude` totals ~620 KB; packing it saves nothing measurable and costs
`grep` and `$EDITOR`. `:set memory=off|auto|ask` decides whether memories are
WRITTEN, not how they are encoded.

==============================================================================
7. ENCRYPTION, LATER — DESIGNED FOR, NOT BUILT           *storage-encryption*

**Two layers that do not touch.** Compression is the row's, and ours.
Encryption is the file's: SQLCipher encrypts every page, the WAL included —
structure, sizes, timestamps and model names too, which are personal as well.
Compress-then-encrypt then holds by construction (a row is compressed before
its page is encrypted), and the schema above needs no envelope column at all.
(rusqlite's `bundled-sqlcipher` feature — recalled, verify when building.)

- **The key comes from the Vault** (`secrets/vault.rs`): one keychain item.
  Losing it loses the history, so this needs an export/recovery story before
  it ships.
- **The alternative, per-frame AEAD**, encrypts bodies only and leaves the
  graph's shape readable, and needs KEYED ids at rest: a plaintext content hash
  confirms a guessed turn. More code for less protection; only worth it for
  per-episode keys or sharing.
- **The compression oracle** (CRIME — secrets.md §3c `*secrets-entropy*`).
  Secret VALUES are masked at capture and at authoring, so the store should
  not hold them — but a value typed before it was marked stays in history in
  clear, and the `$EDITOR` paths sealed values unmasked until round 18
  (Reviewer E), so the analysis must ASSUME a frame can hold one (and that
  outlines are a second plaintext copy of every first line). Other private text compressed in one frame with
  output an attacker can steer (a fetched page) is the CRIME shape, given
  someone who reads the encrypted file repeatedly while steering. SQLCipher
  shows them page counts, not frame lengths, which is coarser. **Gate: redo
  §3c's analysis before encryption ships**; if it bites, frame untrusted
  output separately or pad.

==============================================================================
8. PLAN AND UNKNOWNS                                            *storage-plan*

1. Lazy turn bodies in the graph (backend-independent; the big one). BUILT
   2026-10-02 (§4).
2. `SqliteStore` under the shared contract tests; bodies zstd-3 per turn.
3. The setting picking the backend, and a converter.
Later, each when earned: image blobs, a dictionary, edit deltas, SQLCipher.

Dependencies: `rusqlite` (bundled SQLite) and `zstd` — both C under the hood,
both very widely used, both what the probe ran.

Unknowns that would change this: Shannon's own turns may not look like Claude
Code's (no data yet — the store will say after daily driving); cold-cache and
many-writer timings were not measured; how often large turns get edited
decides whether deltas are ever worth building.
