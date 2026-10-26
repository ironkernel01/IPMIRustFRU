"""test_defs.py - tools/defs.py over the real defs file and over fixtures.

Run from the repository root:  python -I -m unittest discover -s tools -p test_defs.py -v
"""

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import defs  # noqa: E402


def parse(body):
    """read() over a temporary file holding `body`."""
    with tempfile.TemporaryDirectory() as folder:
        path = Path(folder) / "defs.rs"
        path.write_text(body, encoding="utf-8")
        return defs.read(path)


class RealFileTests(unittest.TestCase):
    def test_known_values(self):
        values = defs.read()
        self.assertEqual(values["IPMB_I2C_HZ"], 100000)
        self.assertEqual(values["IPMC_CONSOLE_BAUD"], 115200)
        self.assertEqual(values["IPMC_PRODUCT_ID"], 0x494B)
        self.assertIs(values["IPMB_BUS_A_ENABLE"], True)
        self.assertEqual(values["IPMC_FW_NAME"], "ipmc-nucleo-f401re")
        self.assertEqual(values["IPMB_MASTER_MODE"], "BitBang")
        self.assertEqual(values["IPMC_TEMP_LC_C"], -10)

    def test_every_item_has_a_value_of_a_known_kind(self):
        for name, value in defs.read().items():
            self.assertIsInstance(value, (int, bool, str), name)


class FixtureTests(unittest.TestCase):
    def test_forms(self):
        got = parse(
            "//! doc\n// c\n/// d\npub const A: u32 = 1_000;\npub const B: u8 = 0x2_0;\n"
            "pub const C: i32 = -7;\npub const D: bool = false;\npub const E: &str = \"a b\";\n"
            "pub const F: usize = A;\n"
            "#[derive(Clone, Copy)]\npub enum M {\n    /// x\n    X,\n    Y,\n}\n"
            "pub const G: M = M::Y;\n\n"
        )
        self.assertEqual(
            got, {"A": 1000, "B": 32, "C": -7, "D": False, "E": "a b", "F": 1000, "G": "Y"}
        )

    def test_rejections_name_the_line(self):
        cases = [
            ("let x = 1;\n", ":1:"),
            ("pub const A: u32 = 1 + 2;\n", ":1:"),
            ("\npub const A: bool = 1;\n", ":2:"),
            ("pub const A: &str = \"a\n\";\n", ":1:"),
            ("pub const A: u32 = B;\n", ":1:"),
            ("pub const A: u32 = 1;\npub const A: u32 = 2;\n", ":2:"),
            ("pub enum M {\n    X(u8),\n}\n", ":2:"),
            ("pub enum M {\n    X,\n", "not closed"),
            ("pub const A: M = N::X;\n", ":1:"),
        ]
        for body, part in cases:
            with self.assertRaises(defs.DefsError, msg=body) as caught:
                parse(body)
            self.assertIn(part, str(caught.exception), body)


if __name__ == "__main__":
    unittest.main()
