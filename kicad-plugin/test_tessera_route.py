"""Tests for the plugin's KiCad-independent logic: python3 -m unittest."""

import json
import tempfile
import unittest
from pathlib import Path

import tessera_route as tr

FOUR_LAYERS = """(kicad_pcb (version 20260206)
\t(layers
\t\t(0 "F.Cu" signal)
\t\t(4 "In1.Cu" signal)
\t\t(6 "In2.Cu" power)
\t\t(2 "B.Cu" signal)
\t\t(9 "F.Adhes" user "F.Adhesive")
\t\t(25 "Edge.Cuts" user)
\t)
)"""


class Layers(unittest.TestCase):
    def test_copper_layers_in_stackup_order_and_the_inner_ones(self):
        layers = tr.copper_layers(FOUR_LAYERS)
        self.assertEqual(layers, ["F.Cu", "In1.Cu", "In2.Cu", "B.Cu"])
        self.assertEqual(tr.inner_layers(layers), ["In1.Cu", "In2.Cu"])

    def test_a_two_layer_board_has_no_inner_layers(self):
        two = '(kicad_pcb (layers (0 "F.Cu" signal) (2 "B.Cu" signal) (25 "Edge.Cuts" user)))'
        self.assertEqual(tr.inner_layers(tr.copper_layers(two)), [])


class Config(unittest.TestCase):
    def test_planes_round_trip_and_other_settings_survive(self):
        with tempfile.TemporaryDirectory() as d:
            self.assertEqual(tr.load_planes(d), {})
            (Path(d) / "tessera.json").write_text('{"other": 1}', encoding="utf-8")
            tr.save_planes(d, {"In2.Cu": "GND", "In1.Cu": "+3.3V"})
            self.assertEqual(tr.load_planes(d), {"In1.Cu": "+3.3V", "In2.Cu": "GND"})
            self.assertEqual(json.loads((Path(d) / "tessera.json").read_text())["other"], 1)

    def test_plane_args_are_sorted_layer_net_pairs(self):
        self.assertEqual(
            tr.plane_args({"In2.Cu": "GND", "In1.Cu": "+3.3V"}),
            ["--plane", "In1.Cu=+3.3V", "--plane", "In2.Cu=GND"],
        )


class Summary(unittest.TestCase):
    def test_summary_keeps_the_routers_result_lines(self):
        log = "$ tessera-cli route ...\nwarning: x\nrouted 72 net(s); 2 failed; 0 skipped\nplane fanouts: 74 pad(s) connected; 0 could not be\nwrote out"
        self.assertEqual(
            tr.summary(log),
            "routed 72 net(s); 2 failed; 0 skipped\nplane fanouts: 74 pad(s) connected; 0 could not be",
        )


if __name__ == "__main__":
    unittest.main()
