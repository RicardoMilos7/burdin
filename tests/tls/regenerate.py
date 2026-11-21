#!/usr/bin/env python3
"""Regenerates the test certificates under tests/s3_tls/conformance/src/certs and the copy the `http` tests use,
tests/s3_http/tls/src/certs (stage 3, issues #262 and #260; spec/tls.md, spec/http-client.md).

Needs the `openssl` command-line tool (3.x). Run it when a certificate must change; the output is committed, so the
conformance tests and `tests/check_tls.py` need no tool to run. Every key made here is a throw-away test key, and the
files are the only place those keys live: nothing here is a secret.

What it writes (PEM):
  root.pem               a test certificate authority (EC P-256)
  inter.pem              an intermediate authority signed by root.pem
  server.pem/.key        a leaf for localhost, example.test, and 127.0.0.1, signed by inter.pem (EC P-256)
  chain.pem              server.pem followed by inter.pem: what a server sends
  rsa.pem/.key           the same names on an RSA 2048 key, signed by root.pem
  ed25519.pem/.key       the same names on an Ed25519 key, signed by root.pem
  wrong_host.pem/.key    a leaf for other.test only, signed by root.pem
  self_signed.pem/.key   a self-signed leaf for localhost (not signed by any authority)
  expired.pem/.key       a leaf for localhost valid only from 2020-01-01 to 2020-01-02, signed by root.pem
  client.pem/.key        a client-authentication leaf signed by root.pem (mutual TLS)
  encrypted.key          a private key encrypted with the password `burdin` (the package must refuse it)

The valid certificates are valid from one year BEFORE the day they are made until 100 years after it, so a machine
whose clock or time zone is off by days (CI runners are) still verifies them, and no test goes stale. Every certificate
is signed with `openssl ca`, which takes both dates. Keys and serial numbers are random, so a regenerated set differs
from the committed one in every byte; the tests depend only on names, validity, and who signed what.

`tests/s3_http/tls/src/certs` gets the files its tests use: root.pem, chain.pem, server.key, wrong_host.pem/.key,
expired.pem/.key, and self_signed.pem/.key. They are copies, so each test package stays self-contained.
"""

import shutil
import subprocess
import sys
import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "s3_tls" / "conformance" / "src" / "certs"
HTTP_OUT = ROOT / "s3_http" / "tls" / "src" / "certs"
HTTP_FILES = ["root.pem", "chain.pem", "server.key", "wrong_host.pem", "wrong_host.key", "expired.pem", "expired.key",
              "self_signed.pem", "self_signed.key"]
SANS = "subjectAltName=DNS:localhost,DNS:example.test,IP:127.0.0.1"
NOW = datetime.now(timezone.utc)
START = (NOW - timedelta(days=365)).strftime("%Y%m%d%H%M%SZ")
END = (NOW + timedelta(days=36500)).strftime("%Y%m%d%H%M%SZ")
IDENTIFIERS = "subjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid"


def run(*args: str, cwd: Path) -> None:
    result = subprocess.run(["openssl", *args], cwd=cwd, capture_output=True, text=True)
    if result.returncode != 0:
        sys.exit(f"openssl {' '.join(args)} failed:\n{result.stderr}")


def write(path: Path, text: str) -> None:
    path.write_text(text, encoding="utf-8", newline="\n")


def new_key(work: Path, name: str, kind: str) -> None:
    if kind == "ec":
        run("genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:P-256", "-out", f"{name}.key", cwd=work)
    elif kind == "rsa":
        run("genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048", "-out", f"{name}.key", cwd=work)
    else:
        run("genpkey", "-algorithm", "ED25519", "-out", f"{name}.key", cwd=work)


def csr(work: Path, name: str, common_name: str) -> None:
    run("req", "-new", "-key", f"{name}.key", "-subj", f"/CN={common_name}", "-out", f"{name}.csr", cwd=work)


def issue(work: Path, name: str, signer: str, extensions: str, start: str = START, end: str = END) -> None:
    """Signs `name.csr` with `openssl ca`, which sets both dates (`x509 -req` can only count forward from now). A
    `signer` of `name` makes the certificate self-signed."""
    write(work / f"{name}.ext", extensions + "\n")
    if signer == name:
        who = ["-selfsign", "-keyfile", f"{name}.key"]
    else:
        who = ["-cert", f"{signer}.pem", "-keyfile", f"{signer}.key"]
    run("ca", "-config", "ca.cnf", "-batch", "-notext", *who, "-startdate", start, "-enddate", end,
        "-extfile", f"{name}.ext", "-in", f"{name}.csr", "-out", f"{name}.pem", cwd=work)


def leaf(work: Path, name: str, kind: str, common_name: str, signer: str, san: str, usage: str = "serverAuth") -> None:
    new_key(work, name, kind)
    csr(work, name, common_name)
    issue(work, name, signer, f"basicConstraints=critical,CA:FALSE\nextendedKeyUsage={usage}\n{IDENTIFIERS}\n{san}")


def main() -> None:
    if shutil.which("openssl") is None:
        sys.exit("the openssl command-line tool is required")
    OUT.mkdir(parents=True, exist_ok=True)
    HTTP_OUT.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix="burdin-tls-"))
    try:
        write(work / "index.txt", "")
        write(work / "serial", "01\n")
        write(work / "ca.cnf", "[ca]\ndefault_ca = CA_default\n[CA_default]\ndir = .\ndatabase = index.txt\n"
              "new_certs_dir = .\nserial = serial\ndefault_md = sha256\npolicy = any\nunique_subject = no\n"
              "[any]\ncommonName = supplied\n")

        # The authorities.
        new_key(work, "root", "ec")
        csr(work, "root", "Burdin Test Root")
        issue(work, "root", "root", "basicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\nsubjectKeyIdentifier=hash")
        new_key(work, "inter", "ec")
        csr(work, "inter", "Burdin Test Intermediate")
        issue(work, "inter", "root", f"basicConstraints=critical,CA:TRUE,pathlen:0\nkeyUsage=critical,keyCertSign,cRLSign\n{IDENTIFIERS}")

        # The valid leaves.
        leaf(work, "server", "ec", "localhost", "inter", SANS)
        write(work / "chain.pem", (work / "server.pem").read_text() + (work / "inter.pem").read_text())
        leaf(work, "rsa", "rsa", "localhost", "root", SANS)
        leaf(work, "ed25519", "ed25519", "localhost", "root", SANS)
        leaf(work, "wrong_host", "ec", "other.test", "root", "subjectAltName=DNS:other.test")
        leaf(work, "client", "ec", "burdin client", "root", "subjectAltName=DNS:client.test", usage="clientAuth")

        # A self-signed leaf.
        new_key(work, "self_signed", "ec")
        csr(work, "self_signed", "localhost")
        issue(work, "self_signed", "self_signed", f"basicConstraints=critical,CA:FALSE\nsubjectKeyIdentifier=hash\n{SANS}")

        # An expired leaf, valid for one day in 2020.
        new_key(work, "expired", "ec")
        csr(work, "expired", "localhost")
        issue(work, "expired", "root", f"basicConstraints=critical,CA:FALSE\nextendedKeyUsage=serverAuth\n{IDENTIFIERS}\n{SANS}",
              start="20200101000000Z", end="20200102000000Z")

        # An encrypted key.
        run("genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:P-256", "-aes256", "-pass", "pass:burdin",
            "-out", "encrypted.key", cwd=work)

        names = ["root.pem", "inter.pem", "server.pem", "server.key", "chain.pem", "rsa.pem", "rsa.key",
                 "ed25519.pem", "ed25519.key", "wrong_host.pem", "wrong_host.key", "self_signed.pem",
                 "self_signed.key", "expired.pem", "expired.key", "client.pem", "client.key", "encrypted.key"]
        for name in names:
            write(OUT / name, (work / name).read_text())
        for name in HTTP_FILES:
            write(HTTP_OUT / name, (work / name).read_text())
        print(f"wrote {len(names)} files to {OUT} and {len(HTTP_FILES)} to {HTTP_OUT}")
        print(f"valid from {START} to {END}")
    finally:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
