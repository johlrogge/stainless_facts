# ADR-001: Retraction identity is (entity, value); occurrences are distinct values
## Status
Accepted, 2026-11-10
## Decision
1. **Liveness rule.** Retraction identity is defined as follows:

   > Identity of an assertion is (entity, value), where value is the whole tagged attribute `{t, v}`, compared with `V: Eq`. A value `v` is live on entity `e` iff the last fact on `(e, v)` in log order is an Assert.
   > - A Retract cancels the live `(e, v)` if there is one; otherwise it is a no-op, not an error.
   > - Asserting an already-live `(e, v)` is redundant for liveness. There is never more than one live copy, so one Retract always removes it.
   > - A fact's timestamp and source describe *that line*: when it was recorded and by whom. On a Retract they describe the retraction, never the assertion it cancels. They are never part of matching.
   > - `V`'s `Eq` is the retract identity, so it must be structural. Payloads that aren't `Eq` (e.g. floats with NaN) are unsuitable for retractable attributes. For unknown attributes, identity is `UnknownAttribute` equality (`t` plus JSON `v`). Note that JSON `1` and `1.0` differ.

2. **Timestamp and source never match.** They are provenance of the line they sit on. A retract's source is the retractor, not the author of the assertion it cancels.
3. **Cardinality-one is a view, not a write-time constraint.** This is the one deliberate departure from Datomic. Stainless writes no implicit retract. The LatestWins "current" value is the most recent live value, so retracting it falls back to the previous live value (for example, retracting a user BPM override restores the scanned BPM). A writer that means "replace" writes `Retract(old)` and `Assert(new)` in one batch.
4. **A redundant re-assert refreshes recency.** It adds no copy, but it refreshes the value's "last asserted" stamp. Bpm 120 (t1), Bpm 130 (t2), Bpm 120 (t3) yields 120.
5. **The folds stay mechanism-only.** `FactAggregator` and the fold functions do not enforce the rule, which is today's contract, so old logs fold unchanged. The rule ships as a reference implementation, `LiveSet` (issue #13, planned for 0.4.0).
6. **Occurrence identity lives in the value or the entity.** To make repeated events (plays) individually retractable, put a writer-minted ULID/UUIDv7 inside the value, e.g. `Played { play: Ulid, via: ByQueue, started_at }` on the track entity. Domain time goes in the value; the line timestamp is record time. "Retract this play" is a by-value retract of that exact value. Use a reified entity (`Play(ulid)` with `PlayOf`/`StartedAt`) only when a play has several independently retractable attributes. Stainless mints no ids and takes no `ulid` dependency, which keeps the core dependency-light and wasm-clean.
7. **No per-fact addresses and no format change.** The 5-element line format is unchanged.

## Why
Stainless had no matching rule for retracts. Issues #13, #14 and #15, raised from the downstream MDMA project, exposed the consequences:

- Equal plays (the same value asserted twice) cannot be retracted individually.
- Matching retracts on `(value, source)` broke after a tool upgrade changed the source, so the retracts silently cancelled nothing.
- The log is immutable and the 5-element line format is a contract. The codec and `peek_entity` reject anything but 5 elements, `Operation` is `Copy` and not `non_exhaustive`.
- Replication today is byte-exact relay of lines, and `FactStore` rejects appends older than its latest timestamp (`StoreError::TimestampOrdering`), so offline replica writes cannot be merged into a primary store at all.

Prior art: Datomic (E/A/V/Tx/Op datoms with redundancy rules), OR-Set CRDTs, and event sourcing. See References.

The rule follows Datomic on E/A/V identity, redundant re-assert, redundant retract, a retraction's Tx/timestamp being the retracting one, and provenance staying out of identity.

## Alternatives considered
Per-fact addressing was weighed against putting an occurrence id in the value (A):

| | (A) occurrence id in value | (B1) offset/StreamPosition | (B2) content hash + tiebreaker | (B3) explicit id/dot per line |
|---|---|---|---|---|
| Old by-value retracts unchanged | yes | yes | yes | yes |
| Survives replication | yes (ULID/UUIDv7 needs no coordination) | only byte-exact prefix relays; breaks on any re-encode (Attr re-emits `"v":null` and reorders keys), compaction, migration or merge | only with canonical JSON (RFC 8785/JCS); the order-index tiebreaker breaks under merge | yes |
| List live items with addresses | yes | readers don't yield per-line positions | hashing on read | needs a new reader API |
| Format impact | none | none | none | breaking: the codec and peek_entity reject anything but 5 elements; `Operation` is Copy and not non_exhaustive |
| Addresses legacy lines | no | yes | yes | no |

- **(A) Occurrence id in the value** — chosen.
- **(B1) Offset addresses** — rejected: fragile under any re-encode, compaction, migration or merge.
- **(B2) Content hash** — rejected: needs canonical JSON, and the tiebreaker breaks under merge.
- **(B3) Explicit id/dot per line** — deferred. It is justified only by multi-writer replication with merge, which needs a dedupe key and convergent ordering. That is a separate ADR. If it ever lands: readers first (release N accepts 5 or 6 elements, N+1 writes 6), and explicit ids only.
- **`(value, source)` matching** — rejected: source is provenance, and it broke after a tool upgrade.
- **Enforcing the rule in `FactAggregator`** — rejected: it would force `V: Eq` on everyone and break intentional multisets.
- **OR-Set add-wins** — equivalent under a single writer; deferred together with multi-writer replication.

## Consequences
**Positive**
- No format change; old logs fold unchanged.
- Replica-safe: nothing depends on byte positions or hashes.
- A duplicated assert is idempotent for liveness.
- Stainless itself carries zero risk: documentation plus an additive module in 0.4.0. Fold dispatch is unchanged.

**Negative**
- Legacy equal-valued occurrences stay unaddressable.
- LatestWins live sets grow until old values are retracted.
- MDMA must migrate per attribute (scalar to LatestWins, set to Union, occurrence to a new id-carrying tag), gated by a differential fold of the production log, old versus new.
- Old retracts written after a tool upgrade cancelled nothing under `(value, source)` matching. Under value-only matching they take effect. MDMA decides: accept it as a bug fix with a one-off report (recommended), or apply a cutoff inside MDMA's aggregator, never in stainless.
- Issue #14 lineage retracts need one retract per origin entity: a Retract cancels the equal live value on the entity it is written to, and only there.
- Legacy `TrackStarted(ByQueue)` keeps folding with MDMA's counter semantics. New plays use the id-carrying tag, which old readers see as `Attr::Unknown`. An optional append-only backfill is one `Retract TrackStarted(ByQueue)` per track, then N `Played { play: ulid_from(original_ts), started_at: original_ts }`.

## Follow-ups
- Issue #13: implement `LiveSet<V: Eq, S>` as the single primitive, with LatestWins/EarliestWins/Union as thin views. It carries a `Stamp { at, seq }` (seq is the set's own assert counter) and `Live { value, first, last, source }`; `retract(&value)` cancels the live assertion equal to `value`, if any, and never compares sources or timestamps. Tie order is fold-local and convergent only across byte-identical replicas.
- Issue #14: rework lineage folding on top of `LiveSet`. Split `FoldStrategy` into projection (Latest/Earliest/Union) and merge policy across lineage (KeepTarget/Drop/Merge). Entities in the lineage are not searched on retract; the live listing carries each value's origin entity. Log ordinals are fold-local and never written to the log or used as identity.
- Issue #15: MDMA's per-occurrence plays move to the id-carrying value pattern (Decision 6).
- Docs-only fixes in 0.3.x: rename the `source` parameter of `FactAggregator::retract`/`retract_unknown` to `retracted_by`, and fix examples that cleared a value on a non-matching retract.
- A separate multi-writer replication ADR: timestamp ordering on append, dedupe, convergent order (and B3 if needed).

## References
- Datomic transaction data reference, https://docs.datomic.com/transactions/transaction-data-reference.html. "A retraction that does not match the E/A/V of an existing datom is also redundant." "Datoms are considered redundant if they are equal to an existing datom in every component except for tx-id."
- Datomic data model (the five-tuple E A V Tx Op), https://docs.datomic.com/whatis/data-model.html
- Datomic reified transactions, https://blog.datomic.com/2015/12/reified-transactions.html
- Shapiro et al., CRDTs (2011), https://pages.lip6.fr/Marc.Shapiro/slides/CRDTs%20Google%20Zurich-2011-09.pdf
- Bieniusa et al., RR-8083 (OR-Set), https://perso.lip6.fr/Marc.Shapiro/papers/RR-8083.pdf
