# Provenance ledger

Mandatory, append-as-you-go record of every module implementing a known
algorithm: primary source, reference implementation consulted, derivation,
licence implication. See `AUTOROUTER_PLAN.md` §11.2. No code has been
ported or derived from a third-party source yet — this file is populated in
the same commit as the first module that draws on one.

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
