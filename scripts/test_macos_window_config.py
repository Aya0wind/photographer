"""Static guardrails for native macOS window chrome."""
import json
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[1]


class MacWindowConfigTests(unittest.TestCase):
    def setUp(self):
        self.config = json.loads(
            (ROOT / "src-tauri" / "tauri.macos.conf.json").read_text()
        )
        self.windows = {
            window["label"]: window
            for window in self.config["app"]["windows"]
        }

    def test_main_uses_native_overlay_titlebar_and_traffic_lights(self):
        main = self.windows["main"]
        self.assertTrue(main["decorations"])
        self.assertEqual(main["titleBarStyle"], "Overlay")
        self.assertTrue(main["hiddenTitle"])
        self.assertEqual(main["trafficLightPosition"], {"x": 14, "y": 13})

    def test_splash_remains_decorationless_and_main_starts_hidden(self):
        self.assertFalse(self.windows["splash"]["decorations"])
        self.assertFalse(self.windows["main"]["visible"])

    def test_windows_baseline_keeps_custom_chrome(self):
        windows = json.loads((ROOT / "src-tauri" / "tauri.conf.json").read_text())
        main = next(window for window in windows["app"]["windows"] if window.get("label", "main") == "main")
        self.assertFalse(main["decorations"])


if __name__ == "__main__":
    unittest.main()
