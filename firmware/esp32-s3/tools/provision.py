#!/usr/bin/env python3
"""Prepare private NVS data; never connects to or flashes a device."""
import argparse
import base64
import csv
import getpass
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import subprocess
import sys
import uuid

from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography import x509

ROOT = Path(__file__).resolve().parents[1]
PRIVATE = ROOT / ".private"
P256_ORDER = int("ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551", 16)


def b64(data):
    return base64.b64encode(data).decode("ascii")


def installation(username, password):
    # Ported from gbs-anywhere PR16 cloud.rs; preserve the existing cloud identity.
    seed = hashlib.sha256(b"gbs-anywhere La Marzocco installation v1\0" +
                          username.strip().lower().encode() + b"\0" + password.encode()).digest()
    iid = str(uuid.UUID(bytes=hashlib.sha256(seed + b"installation-id").digest()[:16], version=4))
    for index in range(256):
        scalar = int.from_bytes(hashlib.sha256(seed + b"private-key" + bytes([index])).digest(), "big")
        if 0 < scalar < P256_ORDER:
            break
    else:
        raise ValueError("Could not derive a valid installation key")
    key = ec.derive_private_key(scalar, ec.SECP256R1())
    public = key.public_key().public_bytes(serialization.Encoding.DER,
                                           serialization.PublicFormat.SubjectPublicKeyInfo)
    private = key.private_bytes(serialization.Encoding.DER,
                                serialization.PrivateFormat.PKCS8, serialization.NoEncryption())
    secret = hashlib.sha256(f"{iid}.{b64(public)}.{b64(hashlib.sha256(iid.encode()).digest())}".encode()).digest()
    return {"installation_id": iid, "public_key": b64(public), "private_key": b64(private),
            "secret": b64(secret),
            "registration_proof": proof(f"{iid}.{b64(hashlib.sha256(public).digest())}", secret)}


def proof(text, secret):
    work = bytearray(secret)
    for byte in text.encode():
        index = byte % 32
        shift = work[(index + 1) % 32] & 7
        value = byte ^ work[index]
        work[index] = ((value << shift) | (value >> (8 - shift))) & 255
    return b64(hashlib.sha256(work).digest())


def validate_wifi(ssid, password):
    if not 1 <= len(ssid.encode()) <= 32 or "\0" in ssid:
        raise ValueError("WiFi SSID must contain 1-32 bytes and no NUL")
    if not 8 <= len(password.encode()) <= 63 or "\0" in password:
        raise ValueError("WiFi password must contain 8-63 bytes and no NUL")


def build_config(application, pairing, grinder, certificate, ssid, wifi_password):
    validate_wifi(ssid, wifi_password)
    clouds = [entry["settings"] for entry in application["integrations"]
              if entry.get("kind") == "la_marzocco"]
    if len(clouds) != 1:
        raise ValueError("Exactly one La Marzocco integration is required")
    cloud = clouds[0]
    username, password = cloud["username"].strip(), cloud["password"]
    serial = cloud.get("serial") or ""
    for value in (username, password):
        if not isinstance(value, str) or not value or len(value.encode()) > 255 or "\0" in value:
            raise ValueError("Cloud credentials must contain 1-255 bytes and no NUL")
    if not isinstance(serial, str) or len(serial.encode()) >= 64 or "\0" in serial:
        raise ValueError("Machine serial must contain fewer than 64 bytes and no NUL")
    pairing_id = pairing["pairing_id"]
    if type(pairing_id) is not int or not 0 < pairing_id <= 65535:
        raise ValueError("A saved pairing ID in 1-65535 is required")
    ipaddress.IPv4Address(grinder)
    device_id = str(uuid.UUID(pairing["brew_device_id"]))
    if not certificate.strip().startswith("-----BEGIN CERTIFICATE-----") or \
            not certificate.strip().endswith("-----END CERTIFICATE-----") or \
            certificate.count("-----BEGIN") != 1 or len(certificate.encode()) >= 4096 or "\0" in certificate:
        raise ValueError("Provide one PEM certificate smaller than 4096 bytes, without a private key")
    x509.load_pem_x509_certificate(certificate.encode())
    config = {"schema": 1, "wifi_ssid": ssid, "wifi_password": wifi_password,
              "cloud_username": username, "cloud_password": password, "serial": serial,
              "grinder_host": grinder, "grinder_cert": certificate,
              "pairing_id": pairing_id, "brew_device_id": device_id,
              **installation(username, password)}
    if len(json.dumps(config, ensure_ascii=False).encode()) + 1 > 8192:
        raise ValueError("Configuration exceeds the firmware's 8192-byte limit")
    return config


def write_private(path, data):
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(data)
    temporary.chmod(0o600)
    temporary.replace(path)


def prepare(args):
    os.umask(0o077)
    PRIVATE.mkdir(mode=0o700, parents=True, exist_ok=True)
    PRIVATE.chmod(0o700)
    config_file = PRIVATE / "config.json"
    if args.reuse_wifi:
        old = json.loads(config_file.read_text())
        ssid, wifi_password = old["wifi_ssid"], old["wifi_password"]
    else:
        if not sys.stdin.isatty():
            raise ValueError("Run in an interactive terminal to enter the WiFi password privately")
        print("Preparing local configuration; no network requests or flashing.")
        ssid = input("2.4 GHz WiFi SSID: ")
        wifi_password = getpass.getpass("WiFi password (hidden): ")
    validate_wifi(ssid, wifi_password)
    application = json.loads(args.cloud_config.read_text())
    pairing = json.loads(args.pairing_file.read_text())
    config = build_config(application, pairing, args.grinder,
                          args.grinder_cert.read_text(), ssid, wifi_password)
    write_private(config_file, json.dumps(config, ensure_ascii=False) + "\n")
    nvs_csv = PRIVATE / "config.csv"
    with nvs_csv.open("w", newline="") as handle:
        writer = csv.writer(handle)
        writer.writerows([("key", "type", "encoding", "value"),
                          ("gbs_cfg", "namespace", "", ""),
                          ("config", "file", "string", str(config_file)),
                          ("active", "data", "u8", "0")])
    result = subprocess.run([sys.executable, "-m", "esp_idf_nvs_partition_gen", "generate",
                             str(nvs_csv), str(PRIVATE / "config-nvs.bin"), "0x16000"],
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    write_private(PRIVATE / "nvs-generation.log", result.stdout)
    if result.returncode:
        raise ValueError("NVS generation failed; see .private/nvs-generation.log")
    print("Configuration ready. Pairing identity preserved; first boot uses monitor mode.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cloud-config", type=Path, required=True)
    parser.add_argument("--pairing-file", type=Path, required=True)
    parser.add_argument("--grinder", required=True, help="Grinder IPv4 address")
    parser.add_argument("--grinder-cert", type=Path, required=True, help="Trusted grinder certificate in PEM format")
    parser.add_argument("--reuse-wifi", action="store_true")
    try:
        prepare(parser.parse_args())
    except (ValueError, TypeError, AttributeError, KeyError, OSError, EOFError) as error:
        print(f"Preparation failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
