#!/usr/bin/env python3
"""Offline tests: ViaProxy Termux shell launcher, config edits and input safety.

Never download or run third-party JARs from CI while testing this helper.
"""
import os
import pathlib
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = ROOT / 'packaging/android/legacy-proxy/start-termux.sh'


class TermuxHelperTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        root = pathlib.Path(self._tmp.name)
        stub = root / 'bin'
        stub.mkdir()
        home = root / 'home'
        home.mkdir()
        self.cfgdir = home / '.local/share/horizon-189-proxy'
        self.cfgdir.mkdir(parents=True)
        (self.cfgdir / 'ViaProxy.jar').write_text('offline test placeholder')
        (self.cfgdir / 'viaproxy.yml').write_text(
            'bind-address: 0.0.0.0:25568\n'
            'target-address: example.org:25565\n'
            'target-version: 1.21.11\n'
            'auth-method: NONE\n'
            'proxy-online-mode: false\n'
        )
        for binary, script in {
            'java': '#!/bin/sh\nprintf "java called: %s\\n" "$*"\n',
            'curl': '#!/bin/sh\nexit 0\n',
            'jq': '#!/bin/sh\nexit 0\n',
        }.items():
            executable = stub / binary
            executable.write_text(script)
            executable.chmod(0o755)
        self.env = {
            **os.environ,
            'HOME': str(home),
            'PATH': f'{stub}{os.pathsep}{os.environ.get("PATH", "")}',
            'PREFIX': '/data/data/com.termux/files/usr',
        }

    def call(self, *args):
        return subprocess.run(
            ['bash', str(SCRIPT), *args], capture_output=True, text=True,
            env=self.env, timeout=10, check=False,
        )

    def test_updates_three_fields_without_leaking_bind(self):
        result = self.call('mc.example.org:25565')
        self.assertEqual(result.returncode, 0, result.stderr)
        cfg = (self.cfgdir / 'viaproxy.yml').read_text()
        self.assertIn('bind-address: 127.0.0.1:25568\n', cfg)
        self.assertIn('target-address: mc.example.org:25565\n', cfg)
        self.assertIn('target-version: 1.8.x\n', cfg)
        self.assertIn('auth-method: NONE\n', cfg)
        self.assertIn('proxy-online-mode: false\n', cfg)
        self.assertIn('start viaproxy.yml', result.stdout)

    def test_rejects_shell_and_yaml_injection(self):
        for address in ('bad;echo unsafe', '127.0.0.1:25568', 'abc\nport: 9', 'localhost'):
            with self.subTest(address=address):
                self.assertNotEqual(self.call(address).returncode, 0)

    def test_preserves_account_authentication_setting(self):
        cfg = self.cfgdir / 'viaproxy.yml'
        cfg.write_text(cfg.read_text().replace('auth-method: NONE', 'auth-method: account'))
        self.assertEqual(self.call('another.example.net').returncode, 0)
        self.assertIn('auth-method: account\n', cfg.read_text())

    def test_help_does_not_start_java(self):
        result = self.call('--help')
        self.assertEqual(result.returncode, 0)
        self.assertIn('Usage:', result.stdout)
        self.assertNotIn('java called', result.stdout)


if __name__ == '__main__':
    unittest.main()
