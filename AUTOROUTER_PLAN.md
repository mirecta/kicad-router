# Project Plan — `tessera`: A Modern PCB Autorouter in Rust for KiCad

> **Audience:** an autonomous coding agent (Claude) working with a human reviewer.
> **Status:** planning document. Nothing here is verified against a running system.
> **Rule zero:** this document is a *plan*, not a source of truth about external
> systems. Any claim in here about KiCad's API surface, protobuf schema, file
> format, or DRC semantics **must be verified against the actual system before
> you write code that depends on it.** When the plan and reality disagree,
> reality wins — update this document and note it in `DECISIONS.md`.

---

## 0. Mission

Build a production-quality, DRC-correct PCB autorouter in Rust that ships as a
KiCad plugin and respects the constraints KiCad actually models — net classes,
custom DRC rules, differential pairs, layer stackup, plane references, and
length-matched groups.

### Success criteria (measurable, in priority order)

| # | Criterion | Target |
|---|-----------|--------|
| 1 | **DRC parity** — router output passes KiCad's own DRC | 100% clean on corpus, zero exceptions |
| 2 | **Completion rate** on 4-layer benchmark corpus | ≥ 98% of nets |
| 3 | **Reference-plane integrity** — no signal crosses a plane split without a stitching via | 100% |
| 4 | Via count vs. Freerouting on same board | ≤ 60% |
| 5 | Total trace length vs. Freerouting | ≤ 90% |
| 6 | Runtime, 500-net 4-layer board, 8 cores | < 120 s |
| 7 | Diff pair intra-pair skew | within net class limit |

Criterion 1 is non-negotiable and gates everything else. A router that produces
beautiful output that fails DRC is worse than no router, because it costs the
user more time to fix than to route by hand.

### Non-goals (v1)

- Interactive push-and-shove following the cursor. That requires sub-millisecond
  latency and in-process integration; KiCad already has PNS for this.
- Schematic capture, placement, or autoplacement.
- Field solving / actual impedance computation. We **preserve** the user's
  declared impedance conditions; we do not calculate Z₀. See §7.4.
- RF / microwave structures, flex boards, rigid-flex.
- Analog-aware routing (star grounds, Kelvin connections). Out of scope, but do
  not make it *impossible* — leave the constraint system extensible.

---

## 1. Prior art — read before writing code

**Do not start coding until you have surveyed these.** Produce
`docs/PRIOR_ART.md` summarising what each does, what it gets right, and what we
should take from it. This is milestone M0 and it is not optional.

### Directly overlapping projects

| Project | Language | Why it matters |
|---------|----------|----------------|
| **Topola** (`codeberg.org/topola/topola`) | Rust | Closest existing work: topological rubber-band router + autorouter for PCB, NLnet-funded, GUI + headless CLI. **Read its architecture carefully.** Consider whether contributing is better than competing — flag this to the human reviewer explicitly at end of M0. Note: Topola is MIT and we are GPL, so absorption runs one way only (§11.1). |
| **KiCadRoutingTools** (`github.com/drandyhaas/KiCadRoutingTools`) | Rust + Python | Grid A* octilinear router shipping as a working KiCad 9/10 PCM plugin. Has diff pairs (pose-based A* + Dubins heuristic), rip-up/reroute, trombone length matching, GND return vias, Hungarian-algorithm pad swap. Proves external-process routing is viable. Study its IPC/packaging path in particular. |
| **KiCad PNS** (`pcbnew/router/` in KiCad tree) | C++ | The authoritative implementation of KiCad's collision, shove, and walkaround semantics. Even though we can't link it, its *behaviour* defines what "DRC-correct" means. Read `pns_*.cpp`. |
| **Freerouting** | Java | The incumbent. Understand its shape-based algorithm and, more importantly, catalogue its failure modes — these become our test cases. |
| **toporouter** (gEDA PCB / pcb-rnd) | C | CDT-based topological routing. Old but the foundational algorithm. |
| **TritonRoute / OpenROAD** | C++ | Modern detailed router. IC-specific and gridded, but its **parallel batch scheduling with conflict-graph coloring** and DRC-driven cost model generalise directly. |

### Papers — the algorithmic core

Read these; cite them in code comments where you implement them.

**Global routing / congestion**
- McMurchie & Ebeling, *PathFinder: A Negotiation-Based Performance-Driven Router* (1995). **This is the single most important paper in the plan.** Negotiated congestion is what separates a router that reaches 85% from one that reaches 99%.
- Pan, Chu et al., *FastRoute* series — Steiner tree construction, edge shifting, monotonic routing, 3-bend maze.
- Chu & Wong, *FLUTE* — fast lookup-table rectilinear Steiner minimal tree.
- *CUGR / CUGR2* (ICCAD contest global routers) — probability-based 3D cost, sparse maze patterns.

**Detailed / topological routing**
- Dai, Kong, Sarrafzadeh — rubber-band sketch routing, homotopy-preserving geometrisation (the SURF line of work).
- Ozdal & Wong, *Algorithmic study on length-matching bus routing* — Lagrangian relaxation for length-constrained routing.
- Yan & Wong — network-flow formulations for **BGA escape routing** / ordered escape.
- Shewchuk, *Adaptive Precision Floating-Point Arithmetic and Fast Robust Geometric Predicates* — for the geometry kernel.

**Optional research track (do NOT build this in v1)**
- RL / MCTS-based routing (DeepPCB-style). Note it in `docs/FUTURE.md` and move on. It is a distraction until the classical pipeline works.

---

## 2. Architecture

### 2.1 Deployment model — external process, IPC

**Decision: external plugin, not in-tree C++.** Rationale:

- KiCad has no stable C++ plugin ABI. "Built-in" means maintaining a permanent
  fork (users must build from source → nobody uses it) or upstreaming (multi-year
  social process, and Rust will not be accepted as an in-tree dependency).
- The IPC API launches plugins as standalone processes, is protobuf-based and
  language-agnostic, and is versioned/stable by design.
- Plugins distribute via KiCad's Plugin and Content Manager. One click.
- Crash isolation: a panic in our router does not take down the user's session.
- Batch routing does not need in-process latency (see §2.3).

### 2.2 Crate layout

Cargo workspace. **The engine must have zero KiCad coupling** — it consumes our
own board model only. All KiCad knowledge lives behind adapters.

```
tessera/
├── Cargo.toml                 # workspace
├── crates/
│   ├── tessera-geom/          # geometry kernel: i64 fixed-point, exact predicates,
│   │                          #   polygon boolean/offset, CDT, spatial index
│   ├── tessera-model/         # board model: stackup, nets, pads, obstacles,
│   │                          #   constraint tree. Serde-serialisable.
│   ├── tessera-drc/           # DRC rule engine — KiCad parity. See §3.
│   ├── tessera-global/        # global router: Steiner, congestion, layer assignment
│   ├── tessera-detail/        # detailed router: topological + shove
│   ├── tessera-opt/           # post-pass: via reduction, smoothing, length tuning
│   ├── tessera-engine/        # orchestration, rip-up scheduling, parallel batching
│   ├── tessera-io-kicad/      # IPC client + .kicad_pcb parser fallback
│   ├── tessera-io-dsn/        # Specctra DSN/SES (for benchmarking + other EDA)
│   ├── tessera-cli/           # headless binary — the primary dev interface
│   └── tessera-plugin/        # PCM package, GUI shim
├── corpus/                    # benchmark boards (see §9)
├── docs/
│   ├── PRIOR_ART.md
│   ├── DECISIONS.md           # ADR log — append-only
│   ├── DRC_PARITY.md          # our DRC model vs. KiCad's, gap list
│   └── FUTURE.md
└── xtask/                     # benchmark runner, corpus regression
```

**Dependency rule:** `tessera-geom` depends on nothing internal.
`tessera-model` depends on `geom`. `drc`, `global`, `detail`, `opt` depend on
`geom` + `model`. `engine` depends on all four. `io-*` depend on `model` only.
`cli`/`plugin` depend on `engine` + `io-*`. **No back-edges.** Enforce with a
CI check.

### 2.3 The bulk-fetch invariant

> **Never query KiCad during routing.**

Routing performs millions to hundreds of millions of collision queries. The IPC
socket is request-reply over nng; a round trip costs tens to hundreds of
microseconds. Querying across it would take hours per board.

Therefore the pipeline is rigidly:

1. **Ingest** — bulk-fetch the entire board state in a small, bounded number of
   API calls. Build the in-process model.
2. **Route** — entirely local, native speed, no I/O.
3. **Commit** — write results back in one transaction.

This is not an optimisation; it is a load-bearing architectural constraint.
The cost of it is §3, which is the hardest part of the project.

---

## 3. DRC parity — the hardest problem, do it first

Because we route offline, **we must reimplement KiCad's DRC semantics exactly.**
If our model diverges even slightly, output routes beautifully and then fails
the user's DRC — the single worst outcome.

### 3.1 What must be modelled

- Clearance resolution order and precedence (board → net class → custom rule → local override)
- Net classes, including per-net-class overrides and net class *assignment patterns*
- **KiCad's custom DRC rule language** — this is a full expression evaluator
  (`(rule ... (condition "A.NetClass == 'HV'") (constraint clearance (min 1mm)))`).
  This is the highest-risk item in the entire project.
- Hole-to-hole, hole-to-copper, copper-to-edge clearances
- Track width min/opt/max per constraint scope
- Via diameter/drill constraints, blind/buried/micro via legality against stackup
- Zone connection settings, thermal reliefs, zone fill implications
- Keepout / rule areas (per-item-type exclusions)
- Courtyard rules (affects nothing we route, but affects what's an obstacle)
- Diff pair gap and uncoupled length
- Silk/mask clearances (read-only for us, but must not be broken)

### 3.2 The parity harness — build this before the router

**M1 deliverable, blocking.** A test rig that:

1. Takes a corpus board.
2. Generates candidate geometry (random tracks/vias, and adversarial
   near-boundary cases via `proptest`).
3. Asks *our* DRC engine: violation or not?
4. Asks *KiCad's* DRC: violation or not?
5. Fails the test on any disagreement.

Investigate whether the IPC API can trigger a DRC run and return structured
violations. If it can, this loop is cheap and you should run it in CI on every
commit. If it cannot, fall back to driving `kicad-cli pcb drc --format json`
as a subprocess.

**Do not proceed past M1 until parity is clean on the corpus.** A gap list lives
in `docs/DRC_PARITY.md` with each known divergence marked
`BLOCKING` / `DEGRADED` / `ACCEPTED`.

### 3.3 Fallback if the API doesn't expose enough

Custom DRC rules may not be exposed in evaluable form via IPC. If so, parse
`.kicad_pcb` and the project's `.kicad_dru` file directly (`tessera-io-kicad`
already has a parser path for this reason). Verify early — this determines
whether M1 takes two weeks or two months.

---

## 4. Geometry kernel (`tessera-geom`)

### 4.1 Coordinates — integers, always

KiCad stores coordinates as **integer nanometres**. Exploit this.

- Use `i64` nanometres internally. **No `f64` in the geometry kernel's
  public API.** Floats appear only inside cost heuristics, never in predicates
  or stored geometry.
- Exact orientation/incircle predicates (Shewchuk-style, or a vetted crate).
  Degenerate cases and predicate failures are where routers actually die.
- Arcs: KiCad supports arc tracks. Model them natively; do not silently
  polygonise, or you will fail clearance checks by fractions of a micron.

### 4.2 Components

| Component | Approach | Candidate crates |
|-----------|----------|------------------|
| Polygon boolean + offset | Clipper2 via FFI (mature, integer-exact) | `clipper2` bindings |
| Constrained Delaunay triangulation | Incremental, supports dynamic insert/delete | `spade` |
| Spatial index | R-tree for static obstacles, hybrid grid for dynamic | `rstar` |
| Linear algebra / shapes | only where needed | `nalgebra`, `parry2d` |

**Known risk:** Rust's computational geometry ecosystem is thinner than C++'s
(Clipper2 / Boost.Geometry / CGAL). Budget real time for robustness work you
would get for free in C++. Prefer FFI to a battle-tested C++ kernel over
reimplementing exact polygon offsetting.

### 4.3 Incremental spatial index

The router inserts and removes geometry constantly during rip-up/reroute. A
rebuild-from-scratch index will dominate runtime. Design for
`insert` / `remove` / `query_region` / `nearest` all being cheap, and
benchmark this in isolation (`criterion`) before the router depends on it.

---

## 5. The routing pipeline

Five phases. Each is independently testable and independently benchmarkable.

```
Ingest → Preprocess → Global → Detailed → Optimise → Commit
                         ↑         │
                         └─────────┘
                      rip-up & reroute
```

### 5.1 Phase A — Preprocess

- **Net decomposition:** build a rectilinear Steiner minimal tree per net
  (FLUTE). Multi-pin nets become sets of two-pin subnets. This alone
  substantially beats naive nearest-neighbour ordering.
- **Pad escape / fanout:** BGA and fine-pitch parts need escape routing before
  anything else. Model as network flow (Yan & Wong). This is a distinct
  sub-problem — treat it as such.
- **Plane reference map:** for each signal layer, determine its reference plane
  and compute **plane split regions**. See §7.4 — this is a core data structure,
  not an afterthought.
- **Diff pair identification:** by net name convention (`_P`/`_N`, `+`/`-`,
  `_A`/`_B`) and net class. Build coupled-centerline virtual nets.
- **Constraint resolution:** freeze the effective constraint set per net/segment
  so the inner loop never re-evaluates rules.
- **Protected region ingestion:** build the region map, resolve per-region net
  allowlists, and mark all locked geometry as immovable. See §7.5. Regions must
  exist before global routing so capacity is computed against the *available*
  free space, not the nominal one.
- **Routing scope resolution:** determine the working set of nets (full board,
  selection only, or everything except `no_autoroute`). See §7.5.5.

### 5.2 Phase B — Global routing

Coarse 3D grid (G-cells) over the board × layers. Each G-cell edge has a
**capacity** derived from available width and the effective track+clearance
pitch on that layer.

**Algorithm: PathFinder negotiated congestion.**

```
for iteration in 0..max_iter:
    for each net (ordered by criticality):
        rip up existing route
        route via A* on G-cell graph, cost = base + present_congestion + history
    if no overused edges: converge
    update history_cost for all overused edges   # this is the key step
    increase present_congestion sharpness
```

The history term is what makes it work — nets learn to avoid regions that have
been contested in past iterations, rather than oscillating. Do not skip it.

**Layer assignment** is part of this, not a separate step: cost must include
via cost, and the 3D graph must make layer transitions explicit. Use
preferred-direction bias per layer (H/V alternating) as a soft cost, not a hard
constraint.

**Criticality ordering:** diff pairs and length-matched groups route first;
power/ground last (they often become plane connections instead).

### 5.3 Phase C — Detailed routing

**Primary approach: topological (rubber-band sketch) routing.**

Rationale: not grid-constrained, not 45°-constrained, produces denser layouts
with fewer vias and better EMI characteristics. It's what TopoR and Topola do
and what high-end commercial tools converged on.

1. Build a **constrained Delaunay triangulation** of the free space on each
   layer, with pads/obstacles/keepouts as constraints.
2. Route the *topology*: which side of each obstacle each net passes. This is a
   homotopy class, found by searching the triangulation dual graph within the
   corridor the global router assigned.
3. **Geometrise**: convert the rubber-band sketch into actual copper by pulling
   the path taut subject to clearance, producing arcs/lines.
4. **Legalise**: verify against `tessera-drc`. Any violation feeds back.

**Pragmatic staging (important):** implement a **grid-based octilinear A\***
detailed router first, at M2, as a working baseline. It is simple, it will be
bad, and it gives you an end-to-end pipeline plus a regression baseline months
before the topological router is ready. Keep it behind a feature flag
permanently as a fallback for regions where the topological router fails.

**Shove:** when a route can't fit, attempt to displace existing routes
(push-and-shove) before ripping up. Reference PNS behaviour.

### 5.4 Phase D — Rip-up and reroute scheduling

- Maintain a **conflict graph** of nets whose routing regions overlap.
- Colour it; route independent colour classes **in parallel** (`rayon`).
  This is the TritonRoute batch approach and it is how you get real
  multi-core speedup without nondeterminism.
- **Determinism is mandatory.** Same input + same seed ⇒ byte-identical output,
  regardless of thread count. Test this explicitly in CI. Non-deterministic
  routers are undebuggable.
- Progressive N+1 rip-up: on failure, rip 1 blocker, retry; then 2; up to a cap.

### 5.5 Phase E — Optimise

Run to a fixed point or budget:

- **Via minimisation** — the single biggest quality lever after completion rate.
- Path smoothing / taut-pulling.
- Corner reduction, arc fitting.
- **Length tuning:** trombone/accordion meanders for length-matched groups.
  Use Lagrangian relaxation (Ozdal & Wong) rather than greedy per-net padding.
  Tune at the *point of mismatch*, not at the end of the trace.
- **Return-path stitching:** place GND vias adjacent to signal layer transitions.

---

## 6. Differential pairs

Route as a **coupled centerline**, split into two traces at geometrisation.

Must handle:

- **Gap** and **via gap** from net class.
- **Intra-pair skew** — match P against N *within* the pair, compensated at the
  point of mismatch (typically at a bend or via), not deferred to the trace end.
- **Uncoupled length budget** — how far the pair may separate around an
  obstacle before it counts as a violation.
- **Symmetric breakout** from pads and symmetric via pairs.
- **Return vias** — a GND via adjacent to every diff-pair via pair.

`KiCadRoutingTools` uses a pose-based A* with a Dubins-path heuristic for
orientation-aware centerline routing. That's a sound approach for the grid
baseline; the topological router handles it more naturally as a single
thick net with a post-split step.

---

## 7. Supporting KiCad's feature surface

### 7.1 Must support (v1)

- Arbitrary layer count and stackup (target: correct on 2 and 4 layer; must not
  break on 6–8)
- Net classes + per-net overrides
- Custom DRC rules (see §3 — highest risk)
- Diff pairs
- Length-matched groups
- Blind / buried / micro vias with stackup-legal transitions
- Zones (as obstacles and as connection targets), thermal reliefs
- Keepout / rule areas
- Board edge clearance
- Track width per constraint scope, including impedance-controlled widths
- Arcs
- **Locked items** — respect absolutely; never move, reroute, or rip up (§7.5)
- **Protected regions** with per-net allowlists (§7.5)
- **Partial routing** — route a selection / a net subset only (§7.5)

### 7.2 Should support (v1 if cheap, else v1.1)

- Teardrops (don't generate; don't break)
- Via-in-pad / tented vias
- Multi-board / panel awareness (just don't corrupt it)

### 7.3 Explicit deferrals

Track in `docs/FUTURE.md`: flex/rigid-flex, RF structures, back-drilling,
via stubs, embedded components.

### 7.4 Impedance — preserve, don't compute

**KiCad has no field solver.** The user calculates a trace width themselves and
sets it in a net class. Our job is not to solve for Z₀ but to **preserve the
conditions that make the user's declared width correct**:

1. **Reference plane continuity.** A trace crossing a plane split has undefined
   impedance and a broken return path. Plane splits must be *obstacles* for
   impedance-controlled nets. **This is the flagship feature** — Freerouting
   ignores it entirely, and implementing just this would put us ahead of every
   free autorouter.
2. **Layer changes change the reference.** Every via on a controlled-impedance
   net needs a stitching via nearby, or a same-reference transition.
3. **No necking.** A naive router necks down through tight spots; on an
   impedance-controlled net this is a defect. Width is a hard constraint, not a
   soft one.
4. **Board edge and void proximity** raise impedance — enforce keep-away.
5. **3W adjacent-trace spacing** for coupling control.

Read the stackup (thickness, εr, loss tangent) if the API exposes it — useful
for *reporting* estimated Z₀ to the user, and required if we ever add
verification. Not required for v1 routing.

### 7.5 Protected regions and partial routing

> **Framing for the whole project:** the autorouter's job is not to route the
> board. It is to *finish* the board after the engineer has done the parts that
> require judgement. Design for that, and these features stop being edge cases
> and become the primary workflow.

#### 7.5.1 Why this is mandatory, not optional

Some regions of a board cannot be autorouted by any router, now or ever, because
the governing constraints are **electromagnetic properties of a system**, not
geometric properties of a net. They are not expressible in DRC.

Canonical example — a **switching regulator** (buck / boost / buck-boost), where
the vendor datasheet supplies a recommended layout:

| Constraint | Why no router can satisfy it |
|---|---|
| **Hot loop area** — input cap → high-side FET → low-side FET → return | The commutation loop's *enclosed area* sets parasitic L; L·di/dt is the ringing and radiated EMI. That loop is not a net — it spans three components plus the ground return. The router has no object representing it. |
| **Switch node copper** | Must be short (dv/dt antenna) *and* wide (thermal). A tradeoff weighted by switching frequency and package thermals, not by clearance. |
| **Feedback trace** | Must be Kelvin-sensed at the output cap, kept out of the inductor's magnetic field, and never parallel to or beneath the switch node. "Out of the inductor's field" is not a DRC constraint. |
| **Power ground return** | Input-cap ground and low-side source must share low-impedance copper — a pour geometry, not a trace. |

The same reasoning covers: crystals and oscillators, RF front ends, ADC voltage
references and analog sections, USB / high-speed connector launch geometry,
current-sense resistors with Kelvin connections, and gate-drive loops.

**Do not build "buck converter support."** Build one general mechanism.

#### 7.5.2 The user workflow we must support

1. Engineer places the power stage per the datasheet.
2. Engineer hand-routes the critical nets.
3. Engineer **locks** footprints, tracks, and vias.
4. Engineer draws a **named rule area** as a fence.
5. Router fills in everything else.

Steps 3 and 4 are complementary and both are required. Locking says *what may
not be touched*; the region says *where others may not go*.

#### 7.5.3 Model requirements (`tessera-model`)

A `ProtectedRegion` carries:

- **Polygon + layer set** (may be a subset of layers, not necessarily all copper)
- **Net allowlist** — nets permitted to route inside. Everything else is
  excluded. Note this is an *inverse* keepout: a plain keepout is too blunt,
  since the switch node must be routed *inside* the region.
- **Scoped constraint overrides** — clearance, width, via rules that apply only
  within the region
- **Rip-up policy** — whether the router may reroute permitted nets inside, or
  must treat all existing geometry as frozen

Plus, orthogonally, a per-net **`no_autoroute`** flag: skip entirely, regardless
of region membership.

#### 7.5.4 Locked items — invariant, with its own test

> A locked track, via, or footprint is an **immovable obstacle** and is **never
> a rip-up candidate**, at any recursion depth, under any congestion pressure.

This needs an explicit CI test with an adversarial board: a locked pre-route
directly blocking the only viable corridor, under enough congestion that the
scheduler is strongly tempted to eat it. A progressive N+1 rip-up scheduler will
happily consume locked geometry if the exclusion is not enforced at the
candidate-selection level rather than filtered afterward. Enforce it in the
type system if you can — a separate `Frozen` obstacle class that the rip-up
API cannot accept.

#### 7.5.5 Partial routing

Must support routing a **net subset** and a **selection**, not just whole-board.
This is the mode most real users will work in. Implications:

- Global router must accept pre-existing geometry as fixed capacity consumption,
  not as something to be re-planned.
- Congestion history (PathFinder) must initialise from the existing routes.
- Completion metrics must be computed over the *requested* subset, not the board.
- Must be re-runnable incrementally without degrading previous results.

#### 7.5.6 KiCad mapping — verify at M0

KiCad expresses this through **named rule areas** (formerly keepout zones), which
can disallow tracks, vias, pads, footprints, and zone fills, per layer — plus
**custom DRC rules** conditioned on `insideArea()`. Area membership, net class,
and negation compose; wildcards work on area names. KiCad's own documentation
example uses the form:

```
(rule HV_unshielded
  (constraint clearance (min 2mm))
  (condition "A.NetClass == 'HV' && !A.insideArea('Shield*')"))
```

So a buck-stage fence looks roughly like:

```
(rule buck_exclusion
  (constraint disallow track via)
  (condition "A.insideArea('BuckStage') && A.NetClass != 'Power'"))

(rule fb_keepaway
  (constraint clearance (min 1mm))
  (condition "A.NetClass == 'Feedback' && B.NetClass == 'SwitchNode'"))
```

**Verify all of this empirically** — syntax is version-dependent and there is a
documented history of `insideArea` behaving inconsistently across item types
(see KiCad issues #13947, #8438). Custom rules live in a `.kicad_dru` file,
which is the parser fallback path if IPC does not expose them (§3.3).

Add to the M0 API probe: **can we read named rule areas, their layer sets, their
disallow flags, and the locked state of tracks/vias/footprints?**

#### 7.5.7 Corpus additions

Add to `corpus/`:

- A board with a hand-routed, locked buck stage inside a named rule area
- A board where a locked pre-route blocks the only easy corridor (rip-up trap)
- A board exercising partial routing: half hand-routed, half to be completed
- A board with per-layer rule areas (excluded on inner layers only)

---

## 8. KiCad integration (`tessera-io-kicad`)

### 8.1 Verify these four things first — M0 task

Before architecting around the API, empirically confirm what it exposes:

1. **Board stackup** — layer thicknesses, εr?
2. **Net class diff pair parameters** — gap, via gap, uncoupled limit?
3. **Custom DRC rules** — in any evaluable form?
4. **Named rule areas + locked state** — area polygons, layer sets, disallow
   flags; and the `locked` flag on tracks, vias, and footprints? (§7.5)

Items 3 and 4 are the risky ones. If unavailable, use the `.kicad_pcb` / `.kicad_dru`
parser path. Record findings in `docs/DECISIONS.md` with the KiCad version
tested.

### 8.2 Bindings

Two Rust options exist — evaluate both at M0, pick one, document why:

- `kicad-api-rs` — official, from the KiCad team, but documentation is thin and
  it explicitly wants contributors to keep it current.
- `kicad-ipc-rs` — third-party, async-first, typed models, blocking wrapper.

Whichever you pick, **wrap it behind our own trait** (`BoardSource` /
`BoardSink`) so swapping is a contained change. Never let generated protobuf
types leak into `tessera-model`.

### 8.3 Adapters

- **IPC adapter** → the plugin. Primary shipping path.
- **File adapter** → `.kicad_pcb` parse/write. Fallback for anything IPC
  doesn't expose, and enables CI without a running KiCad.
- **DSN/SES adapter** → benchmark against Freerouting on identical inputs, and
  gives Eagle/EasyEDA users the tool for free.
- **CLI** → headless, scriptable, the primary development interface. Build this
  *first*; do not develop against a GUI.

### 8.4 Packaging

Ship a PCM-installable zip with prebuilt binaries for Windows / macOS / Linux.
A thin Python shim provides the KiCad-side GUI entry point and launches the
Rust process. Cross-compile in CI; ship one archive containing all platform
binaries.

---

## 9. Testing and benchmarking

### 9.1 Corpus

Assemble `corpus/` with, at minimum:

- 5 trivial 2-layer boards (smoke tests, must be 100%)
- 10 real 4-layer designs of varying density
- 2 boards with heavy custom DRC rules
- 2 boards with DDR-class length-matched buses
- 2 boards with BGA escape requirements
- 3 boards that Freerouting is known to fail on
- Adversarial: plane splits, keepouts, blind/buried vias, locked pre-routes

Store as `.kicad_pcb` + expected-metrics JSON. Licence-check anything sourced
from the community before committing it.

### 9.2 Test layers

| Layer | Tool | Gate |
|-------|------|------|
| Geometry predicates | `proptest` | every commit |
| DRC parity vs. KiCad | custom harness (§3.2) | every commit, **blocking** |
| Unit tests per crate | `cargo test` | every commit |
| Corpus regression (completion %, vias, length, runtime) | `xtask bench` | every PR, no regressions |
| Microbenchmarks | `criterion` | tracked, alert on >5% regression |
| Determinism (same seed ⇒ same bytes, varying thread count) | custom | every commit |
| **Locked-item & region invariants** (§7.5.4) — rip-up trap board | custom | every commit, **blocking** |

### 9.3 Metrics to track per corpus board

`completion_rate`, `via_count`, `total_length`, `drc_violations` (must be 0),
`runtime_wall`, `runtime_cpu`, `peak_rss`, `plane_crossings_unstitched`,
`diffpair_skew_max`, `length_match_error_max`,
`locked_items_modified` (**must be 0**), `region_violations` (**must be 0**).

Emit as JSON, commit history to a metrics file, plot trends. **A PR that
improves completion but doubles via count is not obviously an improvement** —
make the tradeoff visible rather than optimising one number.

---

## 10. Milestones

Each milestone ends with: working code on `main`, tests green, a short
`docs/DECISIONS.md` entry, and a written status note to the human reviewer
listing what worked, what didn't, and what you'd change.

| ID | Deliverable | Exit criterion |
|----|-------------|----------------|
| **M0** | Prior-art survey; API capability probe (§8.1); bindings choice; workspace skeleton; GPL boilerplate | `docs/PRIOR_ART.md` written; ADR-0001 records GPL-3.0-or-later; `COPYING` + `NOTICE` in place; all four §8.1 API questions answered with evidence; **explicit recommendation on Topola: contribute or compete, noting the one-way licence consequence** |
| **M1** | `tessera-geom` + `tessera-model` + `tessera-drc` + parity harness | DRC parity clean on full corpus; gap list documented |
| **M2** | Ingest → grid octilinear A* → commit; CLI; **locked-item invariant** (§7.5.4) | End-to-end route of a trivial 2-layer board, DRC-clean, visible in KiCad; rip-up-trap corpus board passes |
| **M2.5** | **Protected regions + partial routing** (§7.5) | Locked buck-stage corpus board: region respected, no foreign nets inside, rest of board completes |
| **M3** | Global router: FLUTE Steiner + PathFinder negotiated congestion + layer assignment | ≥90% completion on 4-layer corpus with grid detailed router |
| **M4** | Topological detailed router (CDT + homotopy + geometrisation) | Beats M2 baseline on via count and length at equal completion |
| **M5** | Parallel rip-up scheduling (conflict graph colouring) + determinism guarantee | Near-linear speedup to 8 cores; byte-identical output across thread counts |
| **M6** | Differential pairs | Corpus diff pairs routed within skew and gap limits |
| **M7** | **Plane-reference awareness + return-path stitching** | Zero unstitched plane crossings; flagship differentiator |
| **M8** | Optimisation pass: via reduction, smoothing, length tuning | Hits all §0 targets |
| **M9** | PCM packaging, GUI shim, docs, cross-platform CI | Installable by a non-developer on all three platforms |

**Realistic scope warning for the agent:** M0–M3 is a substantial project.
M4–M8 is a multi-year effort for a team. If progress stalls, the correct move is
to **narrow the scope to a slice with real standalone value** — BGA escape
routing, or a DDR-class length-matched bus router — where automation genuinely
beats hand routing and where the project can be *finished* rather than
perpetually 70% done. Raise this with the human reviewer rather than grinding.

---

## 11. Porting, licensing, and provenance

Much of this project reimplements published algorithms that already exist as code
in other languages. That is normal and expected. This section governs **how**.

### 11.1 Licensing — DECIDED: GPL-3.0-or-later

> **Decision (locked at plan time): the project is licensed GPL-3.0-or-later.**
> Rationale: KiCad itself is GPL-3.0-or-later; the project's goal is a fully
> open tool; and GPL compatibility maximises what may be legally ported.
> Record this in `docs/DECISIONS.md` as ADR-0001 with this rationale.

Every `Cargo.toml` carries `license = "GPL-3.0-or-later"`. Ship `COPYING` at the
repo root and a `NOTICE` file listing third-party attributions.

#### What this permits

| Source | Licence | May we port? |
|--------|---------|--------------|
| **OpenROAD** (`grt`, `drt`) | BSD-3-Clause | ✅ Yes — permissive, GPL-compatible. Retain attribution in `NOTICE`. Verify per-module; FastRoute arrived via a separate lineage. |
| **Topola** | MIT | ✅ Yes — permissive, GPL-compatible. |
| **KiCad PNS** | GPL-3.0-or-later | ✅ Legally yes. **Technically inadvisable — see below.** |
| **Freerouting** | GPL-3.0 | ✅ Yes. |
| **toporouter** (gEDA PCB) | GPL-2.0-**or-later** | ⚠️ Yes *if* "or later". **Verify the actual header before porting** — GPL-2.0-*only* is incompatible with GPL-3.0. |
| **Papers, theses, patents** | n/a | ✅ Always clean. Algorithms are not copyrightable; only their expression is. |

The one genuine trap is the toporouter/gEDA line. Check the actual file headers,
not the project's summary page, and record the finding in `docs/PROVENANCE.md`.

#### The one-way consequence — accept it knowingly

GPL compatibility runs **one direction**. We may absorb MIT and BSD code; MIT and
BSD projects may **not** absorb ours.

Concretely: **Topola is MIT, so Topola cannot take our code, though we can take
theirs.** If the M0 investigation concludes that merging with Topola is the right
move, this licence choice makes that merge a one-way street — we would have to
contribute under *their* terms, not import them under ours.

This is not a reason to reverse the decision, but it must be surfaced explicitly
in the M0 Topola recommendation rather than discovered later.

#### Patents

Patents are separate from copyright. Reading a patent (e.g. Cadence
US20060242614A1 on iterative topological convergence) teaches the technique
legally, but implementing a *live* claim can infringe regardless of licence.
Most foundational routing patents from the 1990s have expired. If the agent
encounters a patent dated within ~20 years that reads on a planned technique,
**stop and flag it to the human reviewer** rather than deciding independently.

> Not legal advice. GPL-3.0-or-later is a well-understood choice for a KiCad
> plugin, but if anything commercial is ever contemplated, consult a lawyer.

#### The PNS rule — now technical, not legal

> **Still: do not port PNS. Read it.**

Under GPL this is no longer a licensing constraint, but the engineering advice is
unchanged and stands on its own merits.

PNS solves a *different problem* — interactive shove under cursor-latency
budgets. Its data structures are shaped by that constraint and are wrong for
batch autorouting, where the tradeoffs invert (throughput over latency, global
optimality over local responsiveness, restart-friendly over incremental).

Read PNS to learn **what KiCad considers legal** — factual behaviour. Then
implement `tessera-drc` from KiCad's documentation plus empirical testing against
the §3.2 parity harness. A port would inherit an architecture we do not want.

### 11.2 Provenance ledger — mandatory

Maintain `docs/PROVENANCE.md`. For **every** module implementing a known
algorithm, record:

| Field | Example |
|-------|---------|
| Module | `tessera-global::congestion` |
| Algorithm | PathFinder negotiated congestion |
| Primary source | McMurchie & Ebeling, FPGA 1995 (paper) |
| Reference implementation consulted | OpenROAD `grt` (BSD-3) |
| Derivation | Implemented from paper; structure informed by `grt` |
| Licence implication | None — permissive |

If the agent cannot fill in the "Derivation" row honestly, the work is not
finished. This ledger is what makes a licence audit tractable later, and it is
cheap to maintain and expensive to reconstruct.

### 11.3 Do not transliterate — the primary technical trap

> **Line-by-line C++ → Rust translation produces bad Rust.** This is the single
> most likely failure mode when an AI agent does the porting, because agents are
> extremely good at transliteration and transliteration is the wrong output.

Routing code is pointer-graph-heavy, which is exactly where C++ and Rust diverge
most. A direct port yields `Rc<RefCell<T>>` everywhere, borrow-checker fights,
inheritance hierarchies Rust cannot express, and the worst properties of both
languages.

**The idiomatic translation of a C++ pointer graph is an arena plus indices:**

```rust
// NOT this — direct port of a C++ pointer graph:
struct Node { neighbors: Vec<Rc<RefCell<Node>>> }

// THIS:
struct Graph { nodes: Vec<Node>, edges: Vec<Edge> }
#[derive(Copy, Clone, PartialEq, Eq)]
struct NodeId(u32);
```

Use `slotmap` or generational indices where stable handles must survive deletion.
The arena form is *faster* than the C++ original — cache locality, no pointer
chasing — and makes the borrow checker a non-issue rather than an obstacle.

#### Required procedure for any port

The agent must follow this order, and the human reviewer must gate on step 2:

1. Read the reference module.
2. **Write a prose explanation** of the algorithm and its data structures into
   `docs/ports/<module>.md`. No code yet.
3. **Human reviews the prose.** If it is vague or hedged, the port will be a
   transliteration regardless of how the code looks. Send it back.
4. Design Rust data structures independently — arena-based, `Copy` ID types.
5. Implement from the prose explanation, not from the source in front of you.
6. Differential-test against the reference (§11.4).

### 11.4 Port against an oracle

Never port blind. Run the reference implementation and ours on identical input
and diff intermediate structures — congestion maps, Steiner trees, route guides,
layer assignments — not just final output.

FastRoute is the ideal candidate: deterministic, self-contained, and its
intermediates dump easily. Differential testing is how the subtle off-by-one gets
caught now rather than manifesting as an unexplained 5% quality gap six months on.

Keep the oracle harness in `xtask/`. Delete it only when the module is stable and
the reference is no longer being tracked.

### 11.5 Porting order

Sequenced by ratio of value to risk:

| # | Target | Source | Notes |
|---|--------|--------|-------|
| 1 | **FLUTE** | Paper + BSD source | Self-contained, table-driven, ports cleanly. Good calibration exercise for the port procedure itself. |
| 2 | **FastRoute structure** | Paper + OpenROAD `grt` (BSD) | Take the escalation ladder: pattern → monotonic → maze. Differential-test throughout. |
| 3 | **TritonRoute scheduler** | Paper only | Conflict-graph batch colouring + DRC-cost escalation. **Skip its geometry entirely** — gridded, Manhattan, IC design rules, none of which apply. |
| 4 | **Topological router** | Dayan thesis (1997), with toporouter as reference | GPL now permits porting toporouter — **but verify GPL-2.0-or-later first** (§11.1). Prefer the thesis as primary source regardless; toporouter is a 2008 GSoC implementation, not a polished reference. |
| 5 | **DRC engine** | KiCad docs + empirical testing | **Never a port**, for technical reasons (§11.1). |

### 11.6 What transfers from IC routing, and what does not

Roughly 95% of routing literature targets integrated circuits. The distinction
matters constantly:

| | IC | PCB (us) |
|---|---|---|
| Geometry | Manhattan, gridded | any-angle, arcs |
| Net count | millions | hundreds to thousands |
| Constraints | uniform, fixed | rich, per-net, **user-scriptable** |
| Planes | none | central |
| Stackup | fixed by process | designer-chosen |

**Transfers well:** negotiated congestion, Steiner construction, rip-up
scheduling, parallel batch colouring, cost-model design, route-guide interfaces.

**Does not transfer:** essentially all geometry, all design-rule modelling, and
any complexity that exists purely to amortise across millions of nets. At our
scale a simpler implementation may outperform a faithful port — prefer clarity,
measure, and only then optimise.

---

## 12. Engineering standards

### Rust conventions

- Edition 2021+. `#![forbid(unsafe_code)]` in every crate except the FFI
  boundary crate, which must document every `unsafe` block with its invariant.
- **No `unwrap()` / `expect()` / `panic!` in library code.** `thiserror` for
  library errors, `anyhow` at the binary boundary only.
- `clippy::pedantic` on; deviations need an inline `#[allow]` with a reason.
- Public items documented. Non-obvious invariants documented *at the type*.
- Cite the paper in a comment where you implement a published algorithm.

### Process

- Small, reviewable commits. One logical change each.
- Every commit: `cargo fmt --check`, `cargo clippy -- -D warnings`,
  `cargo test`, DRC parity harness.
- `docs/DECISIONS.md` is an **append-only ADR log**: context, options,
  decision, consequences. Never rewrite history in it.
- Benchmark before optimising. Commit the benchmark first, then the
  optimisation, so the delta is in the record.

### Rules for the agent specifically

1. **Verify, don't assume.** Every claim in this document about KiCad's API,
   file format, or DRC behaviour is unverified. Check it. If this document is
   wrong, fix this document.
2. **Never guess at protobuf field names or API surface.** Read the actual
   `.proto` files or generated bindings.
3. **When blocked on an external unknown, stop and ask the human reviewer.**
   Do not invent a plausible-looking API and build on it.
4. **Do not skip M0 or M1.** The temptation to jump straight to routing
   algorithms is strong and it is a trap. A router without DRC parity is
   worthless, and the Topola question needs answering before months are spent.
5. **Prefer deleting code to adding flags.** This project will accumulate
   configuration surface fast; resist it.
6. Keep the engine free of KiCad types. If you find yourself importing an IPC
   type into `tessera-detail`, the design has drifted — stop and fix it.
7. **Never transliterate.** Any port follows the §11.3 procedure: read →
   prose explanation → human review → independent Rust design → implement →
   differential test. Producing Rust that structurally mirrors C++ pointer
   graphs is a defect, not a shortcut.
8. **Update `docs/PROVENANCE.md` in the same commit** as any algorithm
   implementation. Never in a follow-up commit.
9. The project is **GPL-3.0-or-later**. BSD, MIT, and GPL-3-compatible sources
   may be ported with attribution in `NOTICE`. **Verify GPL-2.0-only sources
   before porting** — they are incompatible. Flag any live patent to the human
   reviewer rather than deciding alone.

---

## 13. Risk register

| Risk | Severity | Mitigation |
|------|----------|------------|
| Custom DRC rules not exposed via IPC | **High** | Probe at M0; `.kicad_dru` parser fallback |
| DRC parity proves intractable | **High** | Restrict v1 to boards without custom rules; detect and refuse rather than produce wrong output |
| Rust geometry ecosystem gaps → robustness bugs | High | FFI to Clipper2 rather than reimplementing; integer coords; exact predicates; heavy proptest |
| Topological router is harder than estimated | High | Grid baseline at M2 keeps the project shippable; topological router is an upgrade, not a prerequisite |
| Duplicating Topola's work | Medium | Resolve explicitly at M0 |
| KiCad API changes between versions | Medium | Pin tested version; trait-wrapped bindings; version-detect at runtime |
| Nondeterminism from parallelism | Medium | Determinism test in CI from M5 onward |
| Rip-up scheduler consumes locked geometry | **High** | Enforce at candidate selection, not by post-filter; separate `Frozen` obstacle type; adversarial corpus board from M2 |
| `insideArea()` semantics differ from our model | Medium | Probe at M0; covered by the §3.2 DRC parity harness; known-inconsistent across item types |
| Scope creep into "everything KiCad has" | **High** | §7 tiering; `docs/FUTURE.md` as the pressure valve |
| GPL-2.0-only source ported into a GPL-3.0 project (toporouter/gEDA) | Medium | Verify file headers before porting; record in `docs/PROVENANCE.md` |
| One-way GPL compatibility blocks a future Topola merge | Medium | Accepted knowingly (§11.1); surface in the M0 Topola recommendation |
| Live patent reads on a planned technique | Low | Agent flags any patent under ~20 years old to the human reviewer; never decides alone |
| Agent transliterates C++ instead of designing Rust | **High** | §11.3 procedure with human gate on the prose explanation before any code |
| Silent quality gap vs. reference implementation | Medium | §11.4 differential testing on intermediates, not just final output |

---

## 14. First action

Do **not** write router code. Start M0:

1. Clone and read Topola. Write `docs/PRIOR_ART.md`.
2. Record ADR-0001 in `docs/DECISIONS.md`: **GPL-3.0-or-later**, with the §11.1
   rationale. Add `COPYING`, `NOTICE`, and `license = "GPL-3.0-or-later"` to
   every `Cargo.toml`.
3. Install KiCad 10.x. Enable the IPC API. Write a throwaway probe that answers
   all four §8.1 questions and record real output in `docs/DECISIONS.md`.
4. Report back to the human reviewer with a recommendation on Topola —
   contribute, fork, or build fresh — with reasoning.

Everything else waits on that.
