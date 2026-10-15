# Follow-up audit: correctness and bounded resource use

Base reviewed: `bbabce9c164be02379ff084d70ca8b159f09b574` (2026-10-15).
This is a source review and targeted regression pass, not a security certification.
The earlier findings in [AUDIT.md](AUDIT.md) remain historical records.

## Changes

| Finding | Impact | Resolution |
| --- | --- | --- |
| Unscoped IMAP `EXPUNGE` fallback | A move or draft cleanup could permanently delete other messages already marked `\\Deleted`, including by another client. | Require native `MOVE` or `UIDPLUS` for a move, and `UIDPLUS` for targeted deletion. Reject unsupported operations before mutation. Check the fallback capability once per operation. |
| Retention used the sender's `Date` header | Recently delivered messages with old or missing headers could disappear from the cache after the next sync. | Use the server's `SEARCH SINCE` result for retention, checkpoint the UTC calendar-day cutoff, and reconcile the window when the cutoff changes. Ordinary same-day sync stays incremental. Server messages are never deleted by retention. |
| Duplicate cache insertion rewrote secondary data | A retry could leave the original raw message paired with a different search index and inflate contact-history counts. | Return the existing message ID before MIME parsing, indexing, or contact updates. |
| Duplicate message rendering work | Each render read and parsed the same MIME blob twice and sanitized twice. | Share one raw read/parse and sanitize once per render. |
| Images accumulated before the total limit | Forty individually allowed 5 MiB images could retain roughly 267 MiB of base64 results before most were discarded. | Process results incrementally; share a 25 MiB body budget across all six concurrent requests, including invalid images; deduplicate URLs before the count limit. Encode directly into the final data URL. Transport buffers can still contain an extra in-flight chunk; the cap is not a process RSS guarantee. |
| Automatic HTTP proxies bypassed destination DNS checks | A proxy could resolve the target itself, outside the public-address resolver. | Keep images blocked when HTTP(S)/ALL proxy environment variables are set, and disable automatic proxy use in the image client. Do not silently send around the configured proxy. `NO_PROXY` is conservatively ignored; system VPN tunnels are unaffected. |
| ManageSieve only bounded individual literals/lines | A server, including an unauthenticated peer before STARTTLS, could send many individually valid literals or tokens and exhaust memory. | Bound each complete response to 2 MiB of wire bytes, 16,384 tokens, and 10,000 rows. Charge literals before allocation. |
| Filename length counted characters only | Long Chinese/emoji attachment names exceeded Linux's 255-byte path-component limit. | Keep the existing 120-character cap and add UTF-8-safe byte truncation, retaining a useful extension. |
| Server-rule activation suppressed local-only conditions | A rule rejected by the Sieve exporter could run neither locally nor on the server. | Retain local execution for exporter-rejected rules. Uploaded-script version tracking remains a separate issue below. |
| Async UI responses crossed message/account boundaries | A late response could display the wrong message, attach a draft to the wrong context, or show an attachment confirmation for an old message. | Capture ownership and request generations for navigation, search, replies, source, attachments, sender trust, and linked notes/tasks. |
| Repeated shortcuts could submit duplicate mail | Disabled buttons did not protect keyboard entry into `send()`. | Guard send/save/pick handlers while an operation is already in flight. |
| Autosave used mutable selection/DOM state | Changing task or leaving Notes during debounce could discard edits or apply an old response over newer text. | Snapshot edits, serialize saves, flush on selection/teardown, and ignore stale responses. Sanitize dropped note HTML through the paste path. |
| Repeated UI work | Task initialization loaded twice, independent queries ran serially, and pagination deduplication repeatedly scanned the entire list. | Use one initial reactive load, parallel independent reads, and a `Set` of existing message IDs. |

## Validation and scope

The patch adds frontend tests that execute the actual TypeScript component handlers with
deferred API promises and mocked lifecycle hooks. These cover ordering and ownership;
they do not replace browser/WebKit lifecycle tests. `npm test` is included in CI.

Rust regressions cover targeted IMAP operations against a loopback TLS mock, cache/index
idempotence, image response budgets, ManageSieve aggregate limits, Unicode filenames,
local Sieve fallback, and retention checkpoint behavior. Test certificates and their key
under `tests/fixtures/imap` are intentionally public, localhost-only test fixtures.

Local checks: frontend handler tests, Svelte compilation using the available 5.56.4 compiler
(the lockfile uses a newer version), standalone Rust formatting, and diff whitespace checks.
The local environment lacks the complete Cargo dependency cache. Exact locked frontend
checks/build, Rust compilation/tests/clippy, dependency audits, and the Fedora 44 RPM build
are delegated to the existing GitHub CI; consult the PR checks for their actual results.
Real-provider OAuth, live Dovecot/SMTP, and a real Fedora GNOME/WebKit GUI were not exercised
by this pass. Network integration tests gated by `NEUTRON_IT` remain gated in CI.

## Remaining findings

1. **Rule retries (medium):** automatic rules claim a message before all actions complete,
   and the sync checkpoint advances before rule execution. A failure can permanently skip
   automatic handling. Add durable per-action retry state; blindly retrying can duplicate
   forwards or copies.
2. **Uploaded Sieve versions (medium):** the active flag does not identify the exact rules
   uploaded. Adding or changing an exportable rule requires uploading again. Persist the
   uploaded rule definitions/versions and surface an out-of-date-script state.
3. **OAuth callback reliability (medium):** one idle/reset loopback callback socket can
   abort sign-in rather than accepting a later valid callback until the outer deadline.
4. **Outgoing attachment reads (medium):** `read_picked` checks path metadata then reads
   without a byte limit. A concurrently growing/replaced file can evade that check.
   Use descriptor-based regular-file validation and a bounded read, with a nonblocking
   open strategy for special-file replacement.
5. **Large-cache/rule performance:** address-history backfill materializes all raw messages;
   regex rules compile for each message and message facts parse MIME twice. Stream the
   backfill and introduce a compiled rule plan in a separately benchmarked change.

Protocol references: [UIDPLUS / targeted expunge](https://www.rfc-editor.org/rfc/rfc4315.html),
[IMAP search dates](https://www.rfc-editor.org/rfc/rfc9051.html),
[reqwest proxy configuration](https://docs.rs/reqwest/0.12.28/reqwest/struct.ClientBuilder.html#method.no_proxy).
