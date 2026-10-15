import importlib.util
from pathlib import Path
import unittest
import argparse
from datetime import datetime
import json
import tempfile
from unittest.mock import patch
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import NameOID

spec = importlib.util.spec_from_file_location("provision", Path(__file__).resolve().parents[1] / "tools/provision.py")
provision = importlib.util.module_from_spec(spec)
spec.loader.exec_module(provision)


class ProvisionTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        key = ec.generate_private_key(ec.SECP256R1())
        name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "synthetic-grinder")])
        cert = (x509.CertificateBuilder().subject_name(name).issuer_name(name)
                .public_key(key.public_key()).serial_number(1)
                .not_valid_before(datetime(2025, 1, 1))
                .not_valid_after(datetime(2035, 1, 1)).sign(key, hashes.SHA256()))
        cls.certificate = cert.public_bytes(serialization.Encoding.PEM).decode()

    def config(self, **overrides):
        args = dict(application={"integrations": [{"kind": "la_marzocco", "settings": {
            "username": "rick@example.org", "password": "hunter2", "serial": ""}}]},
            pairing={"pairing_id": 42, "brew_device_id": "11111111-1111-4111-8111-111111111111"},
            grinder="192.168.1.20", certificate=self.certificate,
            ssid="Example WiFi", wifi_password="example-password")
        args.update(overrides)
        return provision.build_config(**args)

    def test_import_preserves_identity_and_certificate(self):
        c = self.config()
        self.assertEqual(c["pairing_id"], 42)
        self.assertEqual(c["brew_device_id"], "11111111-1111-4111-8111-111111111111")
        self.assertEqual(c["grinder_cert"], self.certificate)
        self.assertEqual(c["installation_id"], "1f7adb07-de9a-4453-941f-1b054dd04a7a")

    def test_bad_inputs_are_rejected_before_nvs_generation(self):
        for pairing_id in (None, True, 0, -1, 65536, 1.5):
            with self.subTest(pairing_id=pairing_id), self.assertRaises(ValueError):
                self.config(pairing={"pairing_id": pairing_id})
        for host in ("example.org", "192.168.1.20/path", "::1"):
            with self.assertRaises(ValueError): self.config(grinder=host)
        for cert in ("not a certificate", self.certificate * 2,
                     self.certificate + "-----BEGIN PRIVATE KEY-----"):
            with self.assertRaises(ValueError): self.config(certificate=cert)
        for password in ("", "x" * 256, "secret\0tail"):
            with self.assertRaises(ValueError):
                self.config(application={"integrations": [{"kind": "la_marzocco", "settings": {
                    "username": "rick@example.org", "password": password}}]})

    def test_creates_private_nvs_image_in_monitor_mode(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            (root / "config.json").write_text(json.dumps({
                "wifi_ssid": "Example WiFi", "wifi_password": "example-password"}))
            cloud = root / "cloud.json"
            cloud.write_text(json.dumps({"integrations": [{"kind": "la_marzocco", "settings": {
                "username": "rick@example.org", "password": "hunter2"}}]}))
            pairing = root / "pairing.json"
            pairing.write_text(json.dumps({"pairing_id": 42,
                "brew_device_id": "11111111-1111-4111-8111-111111111111"}))
            cert = root / "cert.pem"; cert.write_text(self.certificate)
            args = argparse.Namespace(reuse_wifi=True, cloud_config=cloud, pairing_file=pairing,
                                      grinder="192.168.1.20", grinder_cert=cert)
            with patch.object(provision, "PRIVATE", root): provision.prepare(args)
            self.assertEqual((root / "config-nvs.bin").stat().st_size, 0x16000)
            self.assertIn("active,data,u8,0", (root / "config.csv").read_text())
            self.assertEqual((root / "config.json").stat().st_mode & 0o777, 0o600)
            self.assertEqual((root / "config-nvs.bin").stat().st_mode & 0o777, 0o600)

    def test_matches_upstream_installation_vector(self):
        key = provision.installation("rick@example.org", "hunter2")
        self.assertEqual(key["installation_id"], "1f7adb07-de9a-4453-941f-1b054dd04a7a")
        self.assertEqual(key["public_key"], "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEVCFLIfS/hQzZ3TwM1pAwaHVBw5YvJzV4GZbaB5BaViDxBYR+mIzSXprUaOhqPaiBXBfb5CovzvMom1//AGD4PA==")
        self.assertEqual(key["secret"], "nKkcrK3K9+vUSI1k4VD/FTWNPZfbRrM+E0DFDogvf4E=")
        self.assertEqual(key, provision.installation(" Rick@Example.org ", "hunter2"))

    def test_proof_vector(self):
        self.assertEqual(provision.proof("abc", bytes(32)), "P5ynD3LZU7oRYIN0D2ZtOlbpepJdBXQH9vdqJGOfq/o=")

    def test_wifi_byte_limits(self):
        provision.validate_wifi("home", "password")
        for ssid, password in [("", "password"), ("家" * 11, "password"), ("home", "short"), ("home", "x" * 64)]:
            with self.assertRaises(ValueError):
                provision.validate_wifi(ssid, password)


if __name__ == "__main__":
    unittest.main()
