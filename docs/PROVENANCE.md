# Provenance ledger

Mandatory, append-as-you-go record of every module implementing a known
algorithm: primary source, reference implementation consulted, derivation,
licence implication. See `AUTOROUTER_PLAN.md` §11.2.

## Algorithm implementations

| Module | Algorithm | Primary source | Reference implementation consulted | Derivation | Licence implication |
|---|---|---|---|---|---|
| `tessera-global::pathfinder` | Negotiated congestion routing | McMurchie & Ebeling, *PathFinder: A Negotiation-Based Performance-Driven Router* (FPGA 1995) | None — implemented directly from the paper's pseudocode (also summarised in plan §5.2) | Implemented from the paper; no reference implementation read or consulted. Dijkstra (not the paper's original maze router) used for the per-net shortest path, since the grid here is a plain graph, not a maze; the negotiation loop (present congestion + accumulating history cost + growing present-congestion factor per round) follows the paper directly. | None — paper only, no code lineage |
| `tessera-global::steiner` | Rectilinear MST-based Steiner tree approximation (Prim's algorithm over Manhattan distance) | Standard textbook algorithm (Prim 1957 for the MST; the RMST-as-Steiner-approximation technique and its 3/2 Steiner-ratio bound are classical results, not tied to one paper) | None — implemented from first principles as a stand-in for **FLUTE** (Chu & Wong), which plan §11.5 names as the eventual target and a port candidate. **FLUTE itself has not been ported and must not be**, until the human-gated procedure in plan §11.3 (read → prose explanation → human review → independent design → implement → differential test) runs. This function exists so multi-pin nets route with *something* now, without pre-empting that gate. | Original implementation; no port | None |

## Open verification items carried over from M0 prior-art survey

- **toporouter / pcb-rnd licence.** Verified as GPL-2.0-or-later
  (`docs/PRIOR_ART.md`), but only against the gEDA PCB origin
  (`github.com/russdill/pcb` mirror, `src/toporouter.c:11`,
  `src/toporouter.h:10-11`). pcb-rnd's own repository (`repo.hu`) was
  unreachable during the M0 survey. **If a future port draws from pcb-rnd's
  tree specifically rather than upstream gEDA PCB, re-verify the license
  header there before porting** — forks can diverge or relicense.
- **`insideArea` vs `intersectsArea` semantics.** KiCad's custom-rule
  expression language exposes both predicates; the plan (§7.5.6) notes a
  documented history of inconsistent behavior across item types (KiCad
  issues #13947, #8438). Must be empirically tested per item type in the M1
  DRC parity harness before `tessera-drc`'s expression evaluator treats them
  as interchangeable-but-different in any specific way.
