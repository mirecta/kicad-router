"""KiCad 10 IPC API action: autoroute the open board with tessera.

The open board (unsaved changes included) is saved as a copy to a temporary
folder, routed there by `tessera-cli`, and the new tracks, vias and plane
zones are created in the open board as a single commit — one Ctrl+Z undoes
the whole routing. The board file itself is never written by this script.

`tessera-cli` is looked up in `$TESSERA_CLI`, then `bin/` next to this
script, then `$PATH`. Plane layers come from `tessera.json` next to the
project, e.g. `{"planes": {"In1.Cu": "+3.3V", "In2.Cu": "GND"}}`. The
router's output is written to `tessera.log` in the project folder.
"""

import json
import os
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


def plane_args(project_dir: str) -> list:
    """`--plane LAYER=NET` arguments from the project's `tessera.json`."""
    config = Path(project_dir) / "tessera.json"
    if not config.exists():
        return []
    planes = json.loads(config.read_text(encoding="utf-8")).get("planes", {})
    args = []
    for layer, net in sorted(planes.items()):
        args += ["--plane", f"{layer}={net}"]
    return args


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


def main() -> int:
    from kipy import KiCad

    board = KiCad().get_board()
    project_dir = board.get_project().path
    log_path = Path(project_dir) / "tessera.log"
    with tempfile.TemporaryDirectory(prefix="tessera-") as workdir:
        board_copy = os.path.join(workdir, "board.kicad_pcb")
        board.save_as(board_copy, overwrite=True, include_project=True)
        items = route_copy(board_copy, plane_args(project_dir), log_path)

    nets_by_name = {net.name: net for net in board.get_nets()}
    new_items = build_items(items, nets_by_name)
    if not new_items:
        return 0
    commit = board.begin_commit()
    try:
        board.create_items(new_items)
    except Exception:
        board.drop_commit(commit)
        raise
    board.push_commit(commit, "tessera: autoroute")
    if items["zones"]:
        board.refill_zones()
    return 0


if __name__ == "__main__":
    sys.exit(main())
