"""Lightweight source integration assertions. Not a substitute for Rust/NDK builds."""
import pathlib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
ANDROID = ROOT / "crates/horizon-client/src/android"


class BridgeLauncherWiringTests(unittest.TestCase):
    def test_mode_is_persisted_and_defaults_to_native(self):
        catalog = (ANDROID / "catalog.rs").read_text()
        for expected in ["pub runtime: RuntimeMode", "#[serde(rename_all =", "#[default]", "Native26_3", "Legacy1_8_9ViaProxy"]:
            self.assertIn(expected, catalog)

    def test_bridge_uses_fixed_loopback_not_arbitrary_target(self):
        bridge = (ANDROID / "legacy_bridge.rs").read_text()
        self.assertIn('pub const LOOPBACK: &str = "127.0.0.1:25568"', bridge)
        self.assertIn('TcpStream::connect_timeout', bridge)
        self.assertIn('RuntimeMode::Legacy1_8_9ViaProxy => LOOPBACK.to_string()', bridge)

    def test_selection_is_connected_to_actual_native_launch(self):
        launcher = (ANDROID / "launcher.rs").read_text()
        pages = (ANDROID / "launcher_pages.rs").read_text()
        main = (ROOT / "crates/horizon-client/src/android_main.rs").read_text()
        self.assertIn('crate::legacy_bridge::check(self.catalog.config.runtime)', launcher)
        self.assertIn('crate::legacy_bridge::server(', launcher)
        self.assertIn('self.catalog.config.runtime.next()', pages)
        self.assertIn('Action::ToggleRuntime', pages)
        self.assertIn('mod legacy_bridge;', main)

    def test_native_ui_has_pojav_style_home(self):
        pages = (ANDROID / "launcher_pages.rs").read_text()
        for label in ['Action::ToggleDrawer', 'Action::CloseDrawer', '"Minecraft Java Edition"', '"PLAY"', '"Files"', '"Logs"']:
            self.assertIn(label, pages)
        self.assertIn('pub(super) fn draw_pages', pages)
        self.assertNotIn('PojavLauncherActivity', pages)


if __name__ == '__main__':
    unittest.main()
