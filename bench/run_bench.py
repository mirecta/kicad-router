#!/usr/bin/env python3
"""Benchmark tessera-cli against Freerouting on real KiCad boards.

Reproduces end to end:
  1. scratch copy of each board (+ .kicad_pro) under $TESSERA_BENCH_OUT/<name>/ (default ~/.cache/tessera-bench),
     footprint graphics on the undefined "Rescue" layer stripped,
  2. an "unrouted" board: every track/arc/via removed with pcbnew, zones
     kept (and refilled), baseline DRC recorded,
  3. Freerouting: DSN export -> headless run under xvfb -> SES import,
  4. tessera-cli route (with --plane for inner layers carrying a single-net zone),
  5. per result: runtime, `kicad-cli pcb drc --refill-zones` unconnected
     count, new violations by type vs. the unrouted baseline, vias, track length,
  6. PNG renders of every result,
and writes results.json and results.md there.

Original board files are only ever read. Usage:
    python3 bench/run_bench.py [--only NAME ...] [--timeout SECONDS]
"""
import argparse
import collections
import json
import os
import re
import shutil
import subprocess
import sys
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# outside /tmp so results survive a reboot
OUT = os.environ.get("TESSERA_BENCH_OUT", os.path.expanduser("~/.cache/tessera-bench"))
TESSERA = os.path.join(REPO, "target", "release", "tessera-cli")
FREEROUTING_JAR = os.path.expanduser(
    "~/.local/share/kicad/7.0/3rdparty/plugins/app_freerouting_kicad-plugin/jar/freerouting-1.9.0.jar")
TIMEOUT_S = 600

BOARDS = {
    "ledpixel_shifter": "/home/miro/git/rgbled_shifter/ledpixel_shifter.kicad_pcb",
    "servo-controller": "/home/miro/git/servo-controller/pcb/servo-controller.kicad_pcb",
    "electric-meter": "/home/miro/git/esp32-electric-meter/KiCAD/electric-meter.kicad_pcb",
    "sklennik": "/home/miro/git/esp32-iot-cert-tdoor/KiCAD/sklennik.kicad_pcb",
    "navcompico": "/home/miro/git/navcompico.off/CAD/navcompico/navcompico.kicad_pcb",
    "navcoms": "/home/miro/git/navcoms/CAD/navcoms/navcompico.kicad_pcb",
    "esp32-plc": "/home/miro/git/esp32-plc/Kicad/esp32-plc/esp32-plc.kicad_pcb",
    "royalblue54l-nfc-antenna": os.path.join(REPO, "corpus/2layer-trivial/royalblue54l-nfc-antenna/board.kicad_pcb"),
}


# ---------------------------------------------------------------- helpers

def run(cmd, timeout=None, **kw):
    t0 = time.monotonic()
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout, **kw)
        return p.returncode, p.stdout + p.stderr, time.monotonic() - t0, False
    except subprocess.TimeoutExpired as e:
        out = (e.stdout or b"").decode(errors="replace") if isinstance(e.stdout, bytes) else (e.stdout or "")
        return None, out, time.monotonic() - t0, True


def pcbnew_py(code, *args, timeout=600):
    """Run a pcbnew snippet in a fresh interpreter (pcbnew asserts spam stderr and
    a crash there must not take the harness down). The snippet prints one JSON line."""
    rc, out, _, _ = run([sys.executable, "-c", code, *args], timeout=timeout)
    for line in reversed(out.splitlines()):
        if line.startswith("{"):
            return json.loads(line)
    noise = ("swig/python detected", "assert \"m_choices")
    useful = [l for l in out.splitlines() if not any(n in l for n in noise)]
    raise RuntimeError(f"pcbnew helper failed (rc={rc}):\n" + "\n".join(useful[-15:]))


def copy_project(src_pcb, dst_pcb):
    """Copy the .kicad_pro next to dst_pcb under the matching basename so
    kicad-cli picks up design rules and net classes."""
    src_pro = os.path.splitext(src_pcb)[0] + ".kicad_pro"
    if os.path.exists(src_pro):
        shutil.copy(src_pro, os.path.splitext(dst_pcb)[0] + ".kicad_pro")
    src_dru = os.path.splitext(src_pcb)[0] + ".kicad_dru"
    if os.path.exists(src_dru):
        shutil.copy(src_dru, os.path.splitext(dst_pcb)[0] + ".kicad_dru")


def strip_rescue_layer(text):
    """Remove every (fp_* ...) item whose own (layer "Rescue") is set."""
    out, i, n, removed = [], 0, len(text), 0
    while True:
        j = text.find("(fp_", i)
        if j < 0:
            out.append(text[i:])
            break
        # find the matching close paren, honouring quoted strings
        depth, k, in_str = 0, j, False
        while k < n:
            c = text[k]
            if in_str:
                if c == "\\":
                    k += 1
                elif c == '"':
                    in_str = False
            elif c == '"':
                in_str = True
            elif c == "(":
                depth += 1
            elif c == ")":
                depth -= 1
                if depth == 0:
                    break
            k += 1
        item = text[j:k + 1]
        if re.search(r'\(layer\s+"?Rescue"?\)', item):
            # also drop the leading indentation of that line
            head = text[i:j]
            out.append(head.rstrip(" \t"))
            removed += 1
            i = k + 1
        else:
            out.append(text[i:k + 1])
            i = k + 1
    return "".join(out), removed


# ---------------------------------------------------------------- pcbnew snippets

PREP_CODE = r'''
import os, sys, json, pcbnew
src, dst = sys.argv[1], sys.argv[2]
b = pcbnew.LoadBoard(src)
# keep the removed wrappers alive: letting SWIG collect them crashes pcbnew
removed_items = list(b.Tracks())
for t in removed_items:
    b.Remove(t)
removed = len(removed_items)
pads = {}
for fp in b.GetFootprints():
    for p in fp.Pads():
        if p.GetNetCode() > 0:
            pads.setdefault(p.GetNetname(), 0)
            pads[p.GetNetname()] += 1
nets2 = sum(1 for v in pads.values() if v >= 2)
cu_ids = [l for l in range(pcbnew.PCB_LAYER_ID_COUNT) if pcbnew.IsCopperLayer(l) and b.IsLayerEnabled(l)]
copper = [b.GetLayerName(l) for l in cu_ids]
inner = {}
for z in b.Zones():
    if z.GetIsRuleArea() or z.GetNetCode() <= 0:
        continue
    for l in cu_ids:
        if not z.IsOnLayer(l):
            continue
        name = b.GetLayerName(l)
        if name.startswith("In"):
            inner.setdefault(name, set()).add(z.GetNetname())
# Leave zones *unfilled*: fills computed on a track-less board flood every
# channel, and tessera treats existing fills as obstacles (measured: with fills
# it routed nothing on ledpixel_shifter). Freerouting's DSN export uses zone
# outlines either way, and every DRC below runs with --refill-zones.
for z in b.Zones():
    z.UnFill()
    z.SetIsFilled(False)
b.Save(dst)
print(json.dumps({"removed_track_items": removed, "nets_ge2_pads": nets2,
                  "copper_layers": copper, "pads": sum(pads.values()),
                  "inner_zone_nets": {k: sorted(v) for k, v in inner.items()}}))
sys.stdout.flush(); os._exit(0)
'''

DSN_CODE = r'''
import os, sys, json, pcbnew
b = pcbnew.LoadBoard(sys.argv[1])
ok = pcbnew.ExportSpecctraDSN(b, sys.argv[2])
bad = []
if not ok:
    # diagnose: add footprints back one by one, record the ones the exporter rejects
    fps = list(b.GetFootprints())
    for f in fps:
        b.Remove(f)
    for f in fps:
        b.Add(f)
        if not pcbnew.ExportSpecctraDSN(b, sys.argv[2] + ".probe"):
            bad.append(f.GetReference() + " " + str(f.GetFPID().GetUniStringLibId()))
            b.Remove(f)
print(json.dumps({"ok": bool(ok), "rejected_footprints": bad}))
sys.stdout.flush(); os._exit(0)
'''

SES_CODE = r'''
import os, sys, json, pcbnew
b = pcbnew.LoadBoard(sys.argv[1])
ok = pcbnew.ImportSpecctraSES(b, sys.argv[2])
b.Save(sys.argv[3])
print(json.dumps({"ok": bool(ok)}))
sys.stdout.flush(); os._exit(0)
'''

STATS_CODE = r'''
import os, sys, json, pcbnew
b = pcbnew.LoadBoard(sys.argv[1])
vias = 0; length = 0; segs = 0
for t in b.Tracks():
    if t.GetClass() == "PCB_VIA":
        vias += 1
    else:
        segs += 1; length += t.GetLength()
print(json.dumps({"vias": vias, "track_items": segs, "track_length_mm": round(pcbnew.ToMM(length), 1)}))
sys.stdout.flush(); os._exit(0)
'''


# ---------------------------------------------------------------- measurements

def drc(pcb):
    out = pcb + ".drc.json"
    rc, log, _, _ = run(["kicad-cli", "pcb", "drc", "--refill-zones", "--severity-all",
                         "--format", "json", "-o", out, pcb], timeout=900)
    if not os.path.exists(out):
        raise RuntimeError(f"kicad-cli drc failed on {pcb}: {log[-2000:]}")
    with open(out) as f:
        d = json.load(f)
    by_type = collections.Counter(v["type"] for v in d.get("violations", []))
    return {"unconnected": len(d.get("unconnected_items", [])), "violations_by_type": dict(by_type)}


def new_violations(base, res):
    """Per-type count increase vs. baseline (unconnected excluded; never negative)."""
    out = {}
    for t, c in res["violations_by_type"].items():
        d = c - base["violations_by_type"].get(t, 0)
        if d > 0:
            out[t] = d
    return out


def render(pcb, layers, png):
    svg = png[:-4] + ".svg"
    run(["kicad-cli", "pcb", "export", "svg", "--layers", ",".join(layers + ["Edge.Cuts"]),
         "--mode-single", "--fit-page-to-board", "--exclude-drawing-sheet", "-o", svg, pcb], timeout=300)
    if os.path.exists(svg):
        run(["magick", "-density", "400", "-background", "white", svg, "-flatten", png], timeout=300)
    return png if os.path.exists(png) else None


def measure(name, pcb, base, layers, rt, extra):
    r = {"runtime_s": round(rt, 1), **extra}
    if not os.path.exists(pcb):
        r["error"] = r.get("error") or "no output board"
        return r
    try:
        d = drc(pcb)
        r["unconnected"] = d["unconnected"]
        r["violations_by_type"] = d["violations_by_type"]
        r["new_violations"] = new_violations(base, d)
        r["new_violations_total"] = sum(r["new_violations"].values())
        r.update(pcbnew_py(STATS_CODE, pcb))
        r["png"] = render(pcb, layers, os.path.join(OUT, f"{name}.png"))
    except Exception as e:  # noqa: BLE001 - recorded, not fatal
        r["error"] = str(e)[:500]
    return r


# ---------------------------------------------------------------- routers

def run_freerouting(d, unrouted, timeout):
    dsn, ses = os.path.join(d, "freerouting.dsn"), os.path.join(d, "freerouting.ses")
    out_pcb = os.path.join(d, "freerouting.kicad_pcb")
    for p in (dsn, ses, out_pcb):
        if os.path.exists(p):
            os.remove(p)
    exp = pcbnew_py(DSN_CODE, unrouted, dsn + ".raw")
    if not exp["ok"]:
        return out_pcb, 0.0, {"error": "pcbnew ExportSpecctraDSN failed; footprints it rejects: "
                              + "; ".join(sorted(set(exp["rejected_footprints"]))) or "?"}
    # same sanitising as the Freerouting KiCad plugin: drop characters its parser chokes on
    with open(dsn + ".raw", encoding="utf-8") as f:
        text = re.sub("[Ωµ]", "", f.read())
    with open(dsn, "w", encoding="utf-8") as f:
        f.write(text)
    cmd = ["xvfb-run", "-a", "java", "-jar", FREEROUTING_JAR, "-de", dsn, "-do", ses, "-mp", "100", "-dl"]
    rc, log, rt, timed_out = run(cmd, timeout=timeout, cwd=d)
    with open(os.path.join(d, "freerouting.log"), "w") as f:
        f.write(log or "")
    extra = {"timed_out": timed_out, "exit_code": rc}
    if timed_out:
        subprocess.run(["pkill", "-f", f"-de {dsn}"])
    if not os.path.exists(ses):
        extra["error"] = "timeout, no SES written" if timed_out else f"no SES written (rc={rc})"
        return out_pcb, rt, extra
    if not pcbnew_py(SES_CODE, unrouted, ses, out_pcb)["ok"]:
        extra["error"] = "SES import failed"
    copy_project(unrouted, out_pcb)
    return out_pcb, rt, extra


TESSERA_PASSES = 4


def unfilled_zone_nets(pcb_text):
    """Nets of copper zones with no fill yet: pours the routing starts, whose
    later refills are provisional (`--soft-fill`). Filled ones are the
    user's own and stay hard obstacles."""
    nets, filled = set(), set()
    for m in re.finditer(r'\n\t\(zone\b', pcb_text):
        depth, k, in_str = 0, m.start() + 2, False
        while k < len(pcb_text):
            c = pcb_text[k]
            if in_str:
                if c == "\\":
                    k += 1
                elif c == '"':
                    in_str = False
            elif c == '"':
                in_str = True
            elif c == "(":
                depth += 1
            elif c == ")":
                depth -= 1
                if depth == 0:
                    break
            k += 1
        zone = pcb_text[m.start():k + 1]
        if "(keepout" in zone:
            continue
        net = re.search(r'\(net (?:\d+ )?"([^"]*)"\)', zone)
        if not net or not net.group(1):
            continue
        nets.add(net.group(1))
        if "(filled_polygon" in zone:
            filled.add(net.group(1))
    return sorted(nets - filled)


def run_tessera(d, unrouted, planes, timeout):
    """Route, refill zones (saving the board), route what's still unconnected
    (pour islands, fanouts that found no room, pour islands left without a
    pad) — up to TESSERA_PASSES times,
    while that helps. This is how kicad-mcp's autoroute runs tessera."""
    out_pcb = os.path.join(d, "tessera.kicad_pcb")
    if os.path.exists(out_pcb):
        os.remove(out_pcb)
    current, total_rt, logs, best_unconnected = unrouted, 0.0, [], None
    with open(unrouted, encoding="utf-8") as f:
        soft = unfilled_zone_nets(f.read())
    extra = {"planes": planes, "passes": 0}
    for n in range(1, TESSERA_PASSES + 1):
        nxt = os.path.join(d, f"tessera.pass{n}.kicad_pcb")
        cmd = [TESSERA, "route"]
        for net in soft:
            cmd += ["--soft-fill", net]
        for layer, net in planes.items():
            cmd += ["--plane", f"{layer}={net}"]
        if n > 1:
            # earlier passes' routing may be ripped up again
            cmd += ["--reroutable-from", unrouted]
        cmd += [current, nxt]
        rc, log, rt, timed_out = run(cmd, timeout=max(1, timeout - total_rt))
        total_rt += rt
        logs.append(" ".join(cmd) + "\n" + (log or ""))
        extra.update({"timed_out": timed_out, "exit_code": rc})
        if timed_out or rc != 0:
            if n == 1:
                extra["error"] = "timeout" if timed_out else f"exit code {rc}: {(log or '').strip().splitlines()[-1:]}"
            break
        copy_project(unrouted, nxt)
        run(["kicad-cli", "pcb", "drc", "--refill-zones", "--save-board", "--severity-all",
             "--format", "json", "-o", nxt + ".refill.json", nxt], timeout=900)
        with open(nxt + ".refill.json") as f:
            refill = json.load(f)
        # what's left: unconnected items, then pour islands cut off by the
        # routing (the next pass stitches them with vias)
        left = (len(refill.get("unconnected_items", [])),
                sum(v["type"] == "isolated_copper" for v in refill.get("violations", [])))
        if best_unconnected is not None and left >= best_unconnected:
            break
        shutil.copy(nxt, out_pcb)
        extra["passes"], best_unconnected, current = n, left, nxt
        if left == (0, 0):
            break
    with open(os.path.join(d, "tessera.log"), "w") as f:
        f.write("\n".join(logs))
    copy_project(unrouted, out_pcb)
    return out_pcb, total_rt, extra


# ---------------------------------------------------------------- per board

def bench_board(name, src, timeout, routers=("freerouting", "tessera"), previous=None):
    d = os.path.join(OUT, name)
    os.makedirs(d, exist_ok=True)
    res = {"board": name, "source": src}
    scratch = os.path.join(d, "original.kicad_pcb")
    with open(src, encoding="utf-8") as f:
        text, n_rescue = strip_rescue_layer(f.read())
    with open(scratch, "w", encoding="utf-8") as f:
        f.write(text)
    copy_project(src, scratch)
    res["rescue_items_stripped"] = n_rescue

    unrouted = os.path.join(d, "unrouted.kicad_pcb")
    try:
        info = pcbnew_py(PREP_CODE, scratch, unrouted)
    except Exception as e:  # noqa: BLE001
        res["skipped"] = f"pcbnew could not load/prepare board: {str(e)[-400:]}"
        return res
    copy_project(scratch, unrouted)
    res.update(info)
    res["layers"] = len(info["copper_layers"])
    try:
        res["baseline"] = drc(unrouted)
    except Exception as e:  # noqa: BLE001
        res["skipped"] = f"kicad-cli DRC failed on unrouted board: {str(e)[:300]}"
        return res

    planes = {l: nets[0] for l, nets in sorted(info["inner_zone_nets"].items()) if len(nets) == 1}
    res["planes"] = planes
    layers = info["copper_layers"]

    for router in ("freerouting", "tessera"):
        if router not in routers:
            # keep the earlier measurement for routers not re-run
            if previous and router in previous:
                res[router] = previous[router]
            continue
        print(f"[{name}] {router} ...", flush=True)
        if router == "freerouting":
            pcb, rt, extra = run_freerouting(d, unrouted, timeout)
        else:
            pcb, rt, extra = run_tessera(d, unrouted, planes, timeout)
        res[router] = measure(f"{name}-{router}", pcb, res["baseline"], layers, rt, extra)
    return res


def cell(r, key):
    if r is None or "unconnected" not in r:
        return "-"
    return str(r.get(key, "-"))


def markdown(results):
    hdr = ("| Board | Layers | Nets (>=2 pads) | Baseline unconnected | "
           "FR unconn. | FR new DRC | FR vias | FR length mm | FR time s | "
           "T unconn. | T new DRC | T vias | T length mm | T time s |")
    lines = [hdr, "|" + "---|" * 14]
    notes = []
    for r in results:
        if "skipped" in r:
            notes.append(f"- **{r['board']}** skipped: {r['skipped']}")
            continue
        row = [r["board"], str(r["layers"]), str(r["nets_ge2_pads"]), str(r["baseline"]["unconnected"])]
        for k in ("freerouting", "tessera"):
            x = r.get(k, {})
            if "unconnected" not in x:
                row += ["FAIL"] + ["-"] * 3 + [str(x.get("runtime_s", "-"))]
                notes.append(f"- {r['board']} / {k}: {x.get('error')}")
                continue
            if x.get("error"):
                notes.append(f"- {r['board']} / {k}: {x.get('error')}")
            row += [cell(x, "unconnected"), cell(x, "new_violations_total"), cell(x, "vias"),
                    cell(x, "track_length_mm"), cell(x, "runtime_s")]
        lines.append("| " + " | ".join(row) + " |")
    lines.append("")
    lines.append("New DRC violations by type (count increase vs. unrouted baseline, unconnected excluded):")
    lines.append("")
    for r in results:
        if "skipped" in r:
            continue
        for k in ("freerouting", "tessera"):
            nv = r.get(k, {}).get("new_violations")
            if nv:
                lines.append(f"- {r['board']} / {k}: " + ", ".join(f"{t}={c}" for t, c in sorted(nv.items())))
    if notes:
        lines += ["", "Notes:", ""] + notes
    return "\n".join(lines) + "\n"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--only", nargs="*", help="board names to run")
    ap.add_argument("--timeout", type=int, default=TIMEOUT_S)
    ap.add_argument("--no-build", action="store_true")
    ap.add_argument("--routers", nargs="*", default=["freerouting", "tessera"],
                    help="routers to (re)run; others keep results from the last results.json")
    a = ap.parse_args()
    os.makedirs(OUT, exist_ok=True)
    if not a.no_build:
        env = dict(os.environ, PATH=os.path.expanduser("~/.cargo/bin") + ":" + os.environ["PATH"])
        subprocess.run(["cargo", "build", "--release", "-p", "tessera-cli"], cwd=REPO, env=env, check=True)
    results_path = os.path.join(OUT, "results.json")
    old = {}
    if (a.only or set(a.routers) != {"freerouting", "tessera"}) and os.path.exists(results_path):
        old = {r["board"]: r for r in json.load(open(results_path))}
    results = []
    for name, src in BOARDS.items():
        if a.only and name not in a.only:
            if name in old:
                results.append(old[name])
            continue
        results.append(bench_board(name, src, a.timeout, a.routers, old.get(name)))
        with open(results_path, "w") as f:
            json.dump(results, f, indent=1)
    with open(results_path, "w") as f:
        json.dump(results, f, indent=1)
    md = markdown(results)
    with open(os.path.join(OUT, "results.md"), "w") as f:
        f.write(md)
    print(md)


if __name__ == "__main__":
    main()
