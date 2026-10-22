"""KiCad 10 IPC API action: autoroute the open board with tessera.

The open board (unsaved changes included) is saved as a copy to a temporary
folder, routed there by `tessera-cli`, and the new tracks, vias and plane
zones are created in the open board as a single commit — one Ctrl+Z undoes
the whole routing. The board file itself is never written by this script.

On a board with inner copper layers a small dialog asks, per inner layer,
whether it carries signals or is a plane for some net; the choice is kept in
`tessera.json` next to the project (e.g. `{"planes": {"In1.Cu": "+3.3V",
"In2.Cu": "GND"}}`) and pre-filled next time. `tessera-cli` is looked up in
`$TESSERA_CLI`, then `bin/` next to this script, then `$PATH`. The router's
output is written to `tessera.log` in the project folder.
"""

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def find_cli() -> str:
    env = os.environ.get("TESSERA_CLI")
    if env:
        return env
    here = Path(__file__).resolve().parent
    for name in ("tessera-cli", "tessera-cli.exe"):
        candidate = here / "bin" / name
        if candidate.exists():
            return str(candidate)
    found = shutil.which("tessera-cli")
    if found:
        return found
    raise RuntimeError("tessera-cli not found: set TESSERA_CLI, put it in bin/, or on PATH")


SIGNALS = "(signals)"


def copper_layers(pcb_text: str) -> list:
    """The board's copper layer names, in the order the file lists them
    (KiCad lists them in stackup order: F.Cu, In1.Cu, …, B.Cu)."""
    block = re.search(r"\(layers\s*((?:\([^()]*\)\s*)*)\)", pcb_text)
    if not block:
        return []
    return re.findall(r'\(\s*\d+\s+"([^"]+\.Cu)"\s+(?:signal|power|mixed|jumper)', block.group(1))


# A plane for a net with fewer pads than this makes no sense to offer.
MIN_PLANE_PADS = 3


def plane_candidates(pcb_text: str) -> list:
    """Nets worth offering as a plane: those with at least `MIN_PLANE_PADS`
    pads, most pads first, as (net, pad count)."""
    counts = {}
    pieces = re.split(r"\(pad\s", pcb_text)[1:]
    for piece in pieces:
        net = re.search(r'\(net\s+(?:\d+\s+)?"([^"]*)"\)', piece.split("(pad ", 1)[0])
        if net and net.group(1):
            counts[net.group(1)] = counts.get(net.group(1), 0) + 1
    ranked = sorted(counts.items(), key=lambda kv: (-kv[1], kv[0]))
    return [(net, n) for net, n in ranked if n >= MIN_PLANE_PADS]


def inner_layers(layers: list) -> list:
    return [layer for layer in layers if layer not in ("F.Cu", "B.Cu")]


def load_planes(project_dir: str) -> dict:
    config = Path(project_dir) / "tessera.json"
    if not config.exists():
        return {}
    return json.loads(config.read_text(encoding="utf-8")).get("planes", {})


def save_planes(project_dir: str, planes: dict) -> None:
    config = Path(project_dir) / "tessera.json"
    data = json.loads(config.read_text(encoding="utf-8")) if config.exists() else {}
    data["planes"] = planes
    config.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def plane_args(planes: dict) -> list:
    """`--plane LAYER=NET` arguments for tessera-cli."""
    args = []
    for layer, net in sorted(planes.items()):
        args += ["--plane", f"{layer}={net}"]
    return args


def summary(log_text: str) -> str:
    """The router's own one-line result (and fanout line), for the user."""
    lines = [
        line
        for line in log_text.splitlines()
        if line.startswith(("routed ", "plane fanouts", "added "))
    ]
    return "\n".join(lines) if lines else "tessera finished."


def ask_planes(layers: list, candidates: list, current: dict):
    """A dialog: per inner layer, signals or a plane net (from `candidates`,
    (net, pad count) pairs). Returns the planes to use ({layer: net}), or
    None if the user cancelled."""
    import tkinter as tk
    from tkinter import ttk

    root = tk.Tk()
    root.title("tessera autorouter")
    frame = ttk.Frame(root, padding=12)
    frame.grid()
    ttk.Label(frame, text="Inner copper layers: route signals on them, or use as a plane?").grid(
        row=0, column=0, columnspan=2, sticky="w", pady=(0, 8)
    )
    label_of = {net: f"{net}  ({count} pads)" for net, count in candidates}
    for net in current.values():  # a saved choice stays selectable
        label_of.setdefault(net, net)
    net_of = {label: net for net, label in label_of.items()}
    choices = {}
    for row, layer in enumerate(layers, start=1):
        ttk.Label(frame, text=layer).grid(row=row, column=0, sticky="w", padx=(0, 12))
        saved = current.get(layer)
        value = tk.StringVar(value=label_of[saved] if saved else SIGNALS)
        box = ttk.Combobox(
            frame, textvariable=value, values=[SIGNALS, *label_of.values()], state="readonly", width=32
        )
        box.grid(row=row, column=1, sticky="we", pady=2)
        choices[layer] = value

    result = {"planes": None}

    def route(_event=None):
        result["planes"] = {
            layer: net_of[v.get()] for layer, v in choices.items() if v.get() != SIGNALS
        }
        root.destroy()

    buttons = ttk.Frame(frame)
    buttons.grid(row=len(layers) + 1, column=0, columnspan=2, sticky="e", pady=(12, 0))
    ttk.Button(buttons, text="Cancel", command=root.destroy).grid(row=0, column=0, padx=4)
    ttk.Button(buttons, text="Route", command=route).grid(row=0, column=1)
    root.bind("<Return>", route)
    root.bind("<Escape>", lambda _event: root.destroy())
    root.mainloop()
    return result["planes"]


def show_message(text: str, error: bool = False) -> None:
    import tkinter as tk
    from tkinter import messagebox

    root = tk.Tk()
    root.withdraw()
    (messagebox.showerror if error else messagebox.showinfo)("tessera autorouter", text)
    root.destroy()


def route_copy(board_copy: str, extra_args: list, log_path: Path) -> dict:
    """Runs tessera-cli on a saved copy of the board; returns the new items."""
    workdir = os.path.dirname(board_copy)
    routed = os.path.join(workdir, "routed.kicad_pcb")
    items_path = os.path.join(workdir, "new_items.json")
    command = [find_cli(), "route", *extra_args, "--new-items", items_path, board_copy, routed]
    result = subprocess.run(command, capture_output=True, text=True, check=False)
    log_path.write_text(
        f"$ {' '.join(command)}\n{result.stdout}{result.stderr}", encoding="utf-8"
    )
    if result.returncode != 0:
        raise RuntimeError(f"tessera-cli failed (see {log_path}): {result.stderr.strip()}")
    with open(items_path, encoding="utf-8") as f:
        return json.load(f)


def build_items(items: dict, nets_by_name: dict) -> list:
    """kipy board items for tessera's new tracks, vias and plane zones."""
    from kipy.board_types import Track, Via, Zone
    from kipy.geometry import PolygonWithHoles, PolyLine, PolyLineNode, Vector2
    from kipy.proto.board.board_types_pb2 import VT_THROUGH, ZCS_THERMAL, ZT_COPPER
    from kipy.util.board_layer import layer_from_canonical_name

    created = []
    for t in items["tracks"]:
        track = Track()
        track.start = Vector2.from_xy(*t["start"])
        track.end = Vector2.from_xy(*t["end"])
        track.width = t["width"]
        track.layer = layer_from_canonical_name(t["layer"])
        track.net = nets_by_name[t["net"]]
        created.append(track)
    for v in items["vias"]:
        via = Via()
        via.type = VT_THROUGH
        via.position = Vector2.from_xy(*v["position"])
        via.diameter = v["diameter"]
        via.drill_diameter = v["drill"]
        via.net = nets_by_name[v["net"]]
        created.append(via)
    for z in items["zones"]:
        zone = Zone()
        zone.type = ZT_COPPER
        zone.layers = [layer_from_canonical_name(z["layer"])]
        zone.net = nets_by_name[z["net"]]
        line = PolyLine()
        for x, y in z["outline"]:
            line.append(PolyLineNode.from_xy(x, y))
        line.closed = True
        outline = PolygonWithHoles()
        outline.outline = line
        zone.outline = outline
        zone.clearance = z["clearance"]
        zone.min_thickness = z["min_thickness"]
        zone.connection.zone_connection = ZCS_THERMAL
        # kipy's `thermal_spokes` accessor returns a copy (as of 0.8.0), so
        # setting through it is silently lost — set the message directly.
        spokes = zone.proto.copper_settings.connection.thermal_spokes
        spokes.gap.value_nm = z["thermal_gap"]
        spokes.width.value_nm = z["thermal_bridge_width"]
        created.append(zone)
    return created


def run() -> None:
    from kipy import KiCad

    board = KiCad().get_board()
    project_dir = board.get_project().path
    log_path = Path(project_dir) / "tessera.log"
    nets_by_name = {net.name: net for net in board.get_nets() if net.name}
    with tempfile.TemporaryDirectory(prefix="tessera-") as workdir:
        board_copy = os.path.join(workdir, "board.kicad_pcb")
        board.save_as(board_copy, overwrite=True, include_project=True)
        with open(board_copy, encoding="utf-8") as f:
            pcb_text = f.read()
        inner = inner_layers(copper_layers(pcb_text))
        planes = load_planes(project_dir)
        if inner:
            planes = ask_planes(inner, plane_candidates(pcb_text), planes)
            if planes is None:
                return
            save_planes(project_dir, planes)
        items = route_copy(board_copy, plane_args(planes), log_path)

    new_items = build_items(items, nets_by_name)
    if new_items:
        commit = board.begin_commit()
        try:
            board.create_items(new_items)
        except Exception:
            board.drop_commit(commit)
            raise
        board.push_commit(commit, "tessera: autoroute")
        if items["zones"]:
            board.refill_zones()
    show_message(summary(log_path.read_text(encoding="utf-8")))


def main() -> int:
    try:
        run()
    except Exception as error:  # surfaced to the user, not lost in a log
        show_message(f"tessera failed:\n{error}", error=True)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
