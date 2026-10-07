#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Exercise Linux release packaging without compiling Chromium."""
import hashlib
import pathlib
import shutil
import subprocess
import tarfile
import tempfile
import unittest


class LinuxPackaging(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = pathlib.Path(self.temporary.name)
        scripts = self.root / "tools" / "release"
        scripts.mkdir(parents=True)
        self.script = scripts / "pack-core-linux.sh"
        shutil.copyfile(pathlib.Path(__file__).with_name("pack-core-linux.sh"), self.script)
        (self.root / "Cargo.toml").write_text('[workspace.package]\nversion = "9.8.7"\n')
        self.output = self.root / "core" / "src" / "out" / "linux-x64.noindex"
        self.output.mkdir(parents=True)
        for filename in (
            "chrome", "chrome_crashpad_handler", "libEGL.so", "libGLESv2.so",
            "libvulkan.so.1", "chrome_100_percent.pak", "chrome_200_percent.pak",
            "resources.pak", "icudtl.dat", "v8_context_snapshot.bin",
        ):
            (self.output / filename).write_bytes(b"packaging fixture\n")
        (self.output / "chrome").chmod(0o755)
        (self.output / "locales").mkdir()
        (self.output / "locales" / "en-US.pak").write_bytes(b"locale fixture")

    def package(self):
        return subprocess.run(["bash", str(self.script)], capture_output=True, text=True)

    def test_archive_and_checksum(self):
        result = self.package()
        self.assertEqual(result.returncode, 0, result.stderr)
        archive = self.root / "dist" / "fury-core-9.8.7-linux-x64.tar.xz"
        with tarfile.open(archive) as contents:
            self.assertIn("Fury/locales/en-US.pak", contents.getnames())
            self.assertEqual(contents.getmember("Fury/chrome").mode & 0o111, 0o111)
        expected = hashlib.sha256(archive.read_bytes()).hexdigest()
        self.assertEqual(archive.with_name(archive.name + ".sha256").read_text().split()[0], expected)

    def test_missing_binary_fails(self):
        (self.output / "chrome").unlink()
        result = self.package()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing: chrome", result.stderr)
        self.assertFalse((self.root / "dist").exists())

    def test_missing_locales_fails(self):
        shutil.rmtree(self.output / "locales")
        result = self.package()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing: locales", result.stderr)


if __name__ == "__main__":
    unittest.main()
