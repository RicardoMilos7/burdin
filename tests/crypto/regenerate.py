#!/usr/bin/env python3
"""Regenerates the known-answer fixtures under tests/crypto/ (stage 3, issue #266; spec/crypto.md).

Not part of CI: it needs the `cryptography` package, and the fixtures are committed. It exists so every vector has a
stated origin. A fixture line is `name|field|field|...`; every field is lowercase hex except where a mode says so
(`x<count>:<hex>` repeats a hex chunk). The expected value is always the last field.

Two kinds of vectors:
  * published ones (RFC 4231, 5869, 6070, 7748, 8032, 8439, FIPS 180/197/202, the GCM specification), whose expected
    values are typed in below and ASSERTED against the reference libraries before anything is written, so a typo here
    cannot become a wrong fixture;
  * derived ones (many lengths, key sizes, and block-boundary cases), whose expected values come from OpenSSL through
    Python's `hashlib`, `hmac`, and `cryptography`.

Run it from the repository root: python3 tests/crypto/regenerate.py
"""

import hashlib
import hmac as hmac_mod
import shutil
from pathlib import Path

from cryptography.hazmat.primitives import hashes, padding, poly1305, serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.asymmetric.x25519 import X25519PrivateKey, X25519PublicKey
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
from cryptography.hazmat.primitives.ciphers.aead import AESGCM, ChaCha20Poly1305
from cryptography.hazmat.primitives.kdf.hkdf import HKDF

ROOT = Path(__file__).resolve().parent


def hx(b: bytes) -> str:
    return b.hex() if b else "-"


def rep(chunk: bytes, count: int) -> str:
    return f"x{count}:{chunk.hex()}"


def pattern(n: int, seed: int) -> bytes:
    return bytes(((i * 13 + seed) % 256) for i in range(n))


files: dict[str, list[str]] = {}


def add(path: str, line: str) -> None:
    files.setdefault(path, []).append(line)


# ----- hashes -------------------------------------------------------------------------------------------------

HASHES = {
    "md5": hashlib.md5,
    "sha1": hashlib.sha1,
    "sha224": hashlib.sha224,
    "sha256": hashlib.sha256,
    "sha384": hashlib.sha384,
    "sha512": hashlib.sha512,
    "sha3_224": hashlib.sha3_224,
    "sha3_256": hashlib.sha3_256,
    "sha3_384": hashlib.sha3_384,
    "sha3_512": hashlib.sha3_512,
}

# Published values: the empty string and "abc" (FIPS 180-4 / FIPS 202 examples, RFC 1321 for MD5).
PUBLISHED_HASH = {
    ("sha256", b"abc"): "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    ("sha256", b""): "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    ("sha1", b"abc"): "a9993e364706816aba3e25717850c26c9cd0d89d",
    ("md5", b"abc"): "900150983cd24fb0d6963f7d28e17f72",
    ("md5", b""): "d41d8cd98f00b204e9800998ecf8427e",
    ("sha3_256", b"abc"): "3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532",
    ("sha512", b"abc"): (
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a"
        "2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
    ),
}
for (name, msg), want in PUBLISHED_HASH.items():
    assert HASHES[name](msg).hexdigest() == want, (name, msg)

two_block = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"  # the FIPS 180 two-block message
messages = {
    "empty": b"",
    "abc": b"abc",
    "two_block": two_block,
    "block_minus_1": pattern(63, 1),
    "block": pattern(64, 2),
    "block_plus_1": pattern(65, 3),
    "sha512_block": pattern(128, 4),
    "sha512_block_minus_1": pattern(127, 5),
    "sha3_rate": pattern(136, 6),
    "sha3_rate_plus_1": pattern(137, 7),
    "long": pattern(1000, 8),
}
for name, fn in HASHES.items():
    for label, msg in messages.items():
        add(f"hash/{name}.kat", f"{label}|{name}|{hx(msg)}|{fn(msg).hexdigest()}")
    # one million 'a' (FIPS 180 example) for the two most used
million = b"a" * 1_000_000
add("hash/sha256_million.kat", f"million_a|sha256|{rep(b'a', 1_000_000)}|{hashlib.sha256(million).hexdigest()}")
assert hashlib.sha256(million).hexdigest() == "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
for label, msg in (("empty", b""), ("abc", b"abc"), ("long", pattern(1000, 8))):
    for variant, fn in (("shake128", hashlib.shake_128), ("shake256", hashlib.shake_256)):
        for out_len in (16, 32, 200):
            add("hash/shake.kat", f"{label}_{out_len}|{variant}:{out_len}|{hx(msg)}|{fn(msg).hexdigest(out_len)}")

# ----- HMAC ---------------------------------------------------------------------------------------------------

# RFC 4231 test case 2 for SHA-256, a published value.
assert (
    hmac_mod.new(b"Jefe", b"what do ya want for nothing?", "sha256").hexdigest()
    == "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
)
rfc4231 = [
    ("rfc4231_1", b"\x0b" * 20, b"Hi There"),
    ("rfc4231_2", b"Jefe", b"what do ya want for nothing?"),
    ("rfc4231_3", b"\xaa" * 20, b"\xdd" * 50),
    ("rfc4231_4", bytes(range(1, 26)), b"\xcd" * 50),
    ("rfc4231_6", b"\xaa" * 131, b"Test Using Larger Than Block-Size Key - Hash Key First"),
    (
        "rfc4231_7",
        b"\xaa" * 131,
        b"This is a test using a larger than block-size key and a larger than block-size data. "
        b"The key needs to be hashed before being used by the HMAC algorithm.",
    ),
    ("empty_key", b"", b"message"),
    ("empty_both", b"", b""),
    ("key_is_block", pattern(64, 9), pattern(40, 10)),
    ("key_is_block_plus_1", pattern(65, 9), pattern(40, 10)),
    ("sha3_rate_key", pattern(136, 11), pattern(300, 12)),
]
for name in HASHES:
    for label, key, msg in rfc4231:
        add(f"hmac/{name}.kat", f"{label}|{name}|{hx(key)}|{hx(msg)}|{hmac_mod.new(key, msg, name).hexdigest()}")

# ----- HKDF ---------------------------------------------------------------------------------------------------

HKDF_HASHES = {
    "sha1": hashes.SHA1,
    "sha256": hashes.SHA256,
    "sha384": hashes.SHA384,
    "sha512": hashes.SHA512,
    "sha3_256": hashes.SHA3_256,
}


def hkdf_ref(name: str, salt: bytes, ikm: bytes, info: bytes, length: int) -> bytes:
    # cryptography's HKDF treats an empty salt as "no salt", which RFC 5869 defines as a string of zeros.
    return HKDF(algorithm=HKDF_HASHES[name](), length=length, salt=salt or None, info=info).derive(ikm)


# RFC 5869 test case 1 (published)
t1 = hkdf_ref("sha256", bytes(range(13)), b"\x0b" * 22, bytes(range(0xF0, 0xFA)), 42)
assert t1.hex() == "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
hkdf_cases = [
    ("rfc5869_1", bytes(range(13)), b"\x0b" * 22, bytes(range(0xF0, 0xFA)), 42),
    ("rfc5869_3_no_salt_no_info", b"", b"\x0b" * 22, b"", 42),
    ("long_output", b"salt", b"ikm", b"info", 100),
    ("one_byte", b"s", b"i", b"x", 1),
    ("long_inputs", pattern(100, 1), pattern(100, 2), pattern(100, 3), 80),
]
for name in HKDF_HASHES:
    for label, salt, ikm, info, length in hkdf_cases:
        add(f"hkdf/{name}.kat", f"{label}|{name}|{hx(salt)}|{hx(ikm)}|{hx(info)}|{length}|{hkdf_ref(name, salt, ikm, info, length).hex()}")

# ----- PBKDF2 -------------------------------------------------------------------------------------------------

for count, want in ((1, "0c60c80f961f0e71f3a9b524af6012062fe037a6"), (2, "ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957"),
                    (4096, "4b007901b765489abead49d926f721d065a429c1")):  # RFC 6070 (published)
    assert hashlib.pbkdf2_hmac("sha1", b"password", b"salt", count, 20).hex() == want
for name in ("sha1", "sha256", "sha384", "sha512", "sha3_256"):
    for label, password, salt, iterations, length in (
        ("rfc6070_1", b"password", b"salt", 1, 20),
        ("rfc6070_2", b"password", b"salt", 2, 20),
        ("rfc6070_4096", b"password", b"salt", 4096, 20),
        ("two_blocks", b"password", b"salt", 3, 70),
        ("long_password", pattern(200, 4), b"NaCl", 5, 32),
        ("empty_password", b"", pattern(16, 5), 4, 24),
    ):
        add(f"pbkdf2/{name}.kat", f"{label}|{name}|{hx(password)}|{hx(salt)}|{iterations}|{length}|{hashlib.pbkdf2_hmac(name, password, salt, iterations, length).hex()}")

# ----- ChaCha20, Poly1305, and the AEADs -----------------------------------------------------------------------

SUNSCREEN = (
    b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, "
    b"sunscreen would be it."
)


def chacha_xor(key: bytes, nonce: bytes, counter: int, data: bytes) -> bytes:
    c = Cipher(algorithms.ChaCha20(key, counter.to_bytes(4, "little") + nonce), mode=None).encryptor()
    return c.update(data) + c.finalize()


rfc_key = bytes(range(32))
rfc_nonce = bytes.fromhex("000000000000004a00000000")
ct = chacha_xor(rfc_key, rfc_nonce, 1, SUNSCREEN)
assert ct.hex().startswith("6e2e359a2568f98041ba0728dd0d6981e97e7aec1d4360c20a27afccfd9fae0b")  # RFC 8439 2.4.2
add("chacha20/rfc8439.kat", f"rfc8439_2_4_2|{rfc_key.hex()}|{rfc_nonce.hex()}|1|{SUNSCREEN.hex()}|{ct.hex()}")
for ln in (0, 1, 63, 64, 65, 200):
    for ctr in (0, 1, 7):
        data = pattern(ln, 3)
        key = pattern(32, 4)
        nonce = pattern(12, 5)
        add("chacha20/derived.kat", f"len{ln}_ctr{ctr}|{key.hex()}|{nonce.hex()}|{ctr}|{hx(data)}|{hx(chacha_xor(key, nonce, ctr, data))}")

p_key = bytes.fromhex("85d6be7857556d337f4452fe42d506a80103808afb0db2fd4abff6af4149f51b")
p_msg = b"Cryptographic Forum Research Group"
assert poly1305.Poly1305.generate_tag(p_key, p_msg).hex() == "a8061dc1305136c6c22b8baf0c0127a9"  # RFC 8439 2.5.2
add("poly1305/rfc8439.kat", f"rfc8439_2_5_2|{p_key.hex()}|{p_msg.hex()}|a8061dc1305136c6c22b8baf0c0127a9")
for ln in (0, 1, 15, 16, 17, 31, 32, 33, 100, 1000):
    msg = pattern(ln, 6)
    key = pattern(32, 7)
    add("poly1305/derived.kat", f"len{ln}|{key.hex()}|{hx(msg)}|{poly1305.Poly1305.generate_tag(key, msg).hex()}")
# the all-ones message exercises the final carry and the subtraction of p
add("poly1305/derived.kat", f"all_ff|{pattern(32, 8).hex()}|{('ff' * 48)}|{poly1305.Poly1305.generate_tag(pattern(32, 8), b'\xff' * 48).hex()}")

a_key = bytes(range(0x80, 0xA0))
a_nonce = bytes.fromhex("070000004041424344454647")
a_aad = bytes.fromhex("50515253c0c1c2c3c4c5c6c7")
sealed = ChaCha20Poly1305(a_key).encrypt(a_nonce, SUNSCREEN, a_aad)
assert sealed[-16:].hex() == "1ae10b594f09e26a7e902ecbd0600691"  # RFC 8439 2.8.2
add("aead/chacha20poly1305.kat", f"rfc8439_2_8_2|chacha20poly1305|{a_key.hex()}|{a_nonce.hex()}|{a_aad.hex()}|{SUNSCREEN.hex()}|{sealed.hex()}")


def hchacha(k: bytes, n16: bytes) -> bytes:
    import struct

    def rotl(v, n):
        return ((v << n) & 0xFFFFFFFF) | (v >> (32 - n))

    s = list(struct.unpack("<4I", b"expand 32-byte k")) + list(struct.unpack("<8I", k)) + list(struct.unpack("<4I", n16))

    def qr(a, b, c, d):
        s[a] = (s[a] + s[b]) & 0xFFFFFFFF; s[d] = rotl(s[d] ^ s[a], 16)
        s[c] = (s[c] + s[d]) & 0xFFFFFFFF; s[b] = rotl(s[b] ^ s[c], 12)
        s[a] = (s[a] + s[b]) & 0xFFFFFFFF; s[d] = rotl(s[d] ^ s[a], 8)
        s[c] = (s[c] + s[d]) & 0xFFFFFFFF; s[b] = rotl(s[b] ^ s[c], 7)

    for _ in range(10):
        qr(0, 4, 8, 12); qr(1, 5, 9, 13); qr(2, 6, 10, 14); qr(3, 7, 11, 15)
        qr(0, 5, 10, 15); qr(1, 6, 11, 12); qr(2, 7, 8, 13); qr(3, 4, 9, 14)
    return struct.pack("<8I", *(s[0:4] + s[12:16]))


# the XChaCha20 draft's HChaCha20 test vector (published)
hk = bytes.fromhex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f")
hn = bytes.fromhex("000000090000004a0000000031415927")
assert hchacha(hk, hn).hex() == "82413b4227b27bfed30e42508a877d73a0f9e4d58a74a853c12ec41326d3ecdc"
add("chacha20/hchacha20.kat", f"draft_xchacha|{hk.hex()}|{hn.hex()}|82413b4227b27bfed30e42508a877d73a0f9e4d58a74a853c12ec41326d3ecdc")

for ln in (0, 1, 15, 16, 17, 64, 65, 300):
    key = pattern(32, 1)
    for label, aad in (("noaad", b""), ("aad", pattern(21, 2))):
        msg = pattern(ln, 3)
        nonce12 = pattern(12, 4)
        add("aead/chacha20poly1305.kat", f"len{ln}_{label}|chacha20poly1305|{key.hex()}|{nonce12.hex()}|{hx(aad)}|{hx(msg)}|{ChaCha20Poly1305(key).encrypt(nonce12, msg, aad).hex()}")
        nonce24 = pattern(24, 5)
        sub = hchacha(key, nonce24[:16])
        add("aead/xchacha20poly1305.kat", f"len{ln}_{label}|xchacha20poly1305|{key.hex()}|{nonce24.hex()}|{hx(aad)}|{hx(msg)}|{ChaCha20Poly1305(sub).encrypt(b'\0\0\0\0' + nonce24[16:], msg, aad).hex()}")

# ----- AES ----------------------------------------------------------------------------------------------------


def ecb(key: bytes, block: bytes) -> bytes:
    c = Cipher(algorithms.AES(key), modes.ECB()).encryptor()
    return c.update(block) + c.finalize()


fips_block = bytes.fromhex("00112233445566778899aabbccddeeff")
for size, want in ((16, "69c4e0d86a7b0430d8cdb78070b4c55a"), (24, "dda97ca4864cdfe06eaf70a0ec0d7191"), (32, "8ea2b7ca516745bfeafc49904b496089")):
    fips_key = bytes(range(size))
    assert ecb(fips_key, fips_block).hex() == want  # FIPS 197 appendix C
    add("aes/ecb.kat", f"fips197_c_{size * 8}|ecb|{fips_key.hex()}|-|{fips_block.hex()}|{want}")
for size in (16, 24, 32):
    key = pattern(size, 3)
    iv = pattern(16, 11)
    for ln in (0, 1, 15, 16, 17, 31, 64, 100):
        msg = pattern(ln, 5)
        c = Cipher(algorithms.AES(key), modes.CTR(iv)).encryptor()
        add("aes/ctr.kat", f"len{ln}_{size * 8}|ctr|{key.hex()}|{iv.hex()}|{hx(msg)}|{hx(c.update(msg) + c.finalize())}")
        p = padding.PKCS7(128).padder()
        padded = p.update(msg) + p.finalize()
        c = Cipher(algorithms.AES(key), modes.CBC(iv)).encryptor()
        add("aes/cbc.kat", f"len{ln}_{size * 8}|cbc|{key.hex()}|{iv.hex()}|{hx(msg)}|{(c.update(padded) + c.finalize()).hex()}")
# NIST SP 800-38A F.2.1 CBC-AES128 first block (published)
nist_key = bytes.fromhex("2b7e151628aed2a6abf7158809cf4f3c")
nist_iv = bytes.fromhex("000102030405060708090a0b0c0d0e0f")
nist_pt = bytes.fromhex("6bc1bee22e409f96e93d7e117393172a")
p = padding.PKCS7(128).padder()
c = Cipher(algorithms.AES(nist_key), modes.CBC(nist_iv)).encryptor()
cbc_ct = c.update(p.update(nist_pt) + p.finalize()) + c.finalize()
assert cbc_ct.hex().startswith("7649abac8119b246cee98e9b12e9197d")
add("aes/cbc.kat", f"sp800_38a_f_2_1|cbc|{nist_key.hex()}|{nist_iv.hex()}|{nist_pt.hex()}|{cbc_ct.hex()}")
# NIST SP 800-38A F.5.1 CTR-AES128 first block (published)
nist_ctr_iv = bytes.fromhex("f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff")
c = Cipher(algorithms.AES(nist_key), modes.CTR(nist_ctr_iv)).encryptor()
ctr_ct = c.update(nist_pt) + c.finalize()
assert ctr_ct.hex() == "874d6191b620e3261bef6864990db6ce"
add("aes/ctr.kat", f"sp800_38a_f_5_1|ctr|{nist_key.hex()}|{nist_ctr_iv.hex()}|{nist_pt.hex()}|{ctr_ct.hex()}")
# a counter that wraps its low byte and carries into the next
wrap_iv = bytes.fromhex("000000000000000000000000000000fe")
c = Cipher(algorithms.AES(nist_key), modes.CTR(wrap_iv)).encryptor()
wrap_msg = pattern(80, 3)
add("aes/ctr.kat", f"counter_carry|ctr|{nist_key.hex()}|{wrap_iv.hex()}|{wrap_msg.hex()}|{(c.update(wrap_msg) + c.finalize()).hex()}")

zero_key = bytes(16)
zero_iv = bytes(12)
assert AESGCM(zero_key).encrypt(zero_iv, b"", b"").hex() == "58e2fccefa7e3061367f1d57a4e7455a"  # GCM spec test case 1
assert AESGCM(zero_key).encrypt(zero_iv, bytes(16), b"").hex() == "0388dace60b6a392f328c2b971b2fe78ab6e47d42cec13bdf53a67b21257bddf"
add("aead/aesgcm.kat", f"gcm_spec_1|aesgcm|{zero_key.hex()}|{zero_iv.hex()}|-|-|58e2fccefa7e3061367f1d57a4e7455a")
add("aead/aesgcm.kat", f"gcm_spec_2|aesgcm|{zero_key.hex()}|{zero_iv.hex()}|-|{bytes(16).hex()}|0388dace60b6a392f328c2b971b2fe78ab6e47d42cec13bdf53a67b21257bddf")
for size in (16, 24, 32):
    key = pattern(size, 3)
    nonce = pattern(12, 5)
    for ln in (0, 1, 15, 16, 17, 31, 64, 100):
        for label, aad in (("noaad", b""), ("aad", pattern(ln // 3 + 1, 9))):
            msg = pattern(ln, 5)
            add("aead/aesgcm.kat", f"len{ln}_{size * 8}_{label}|aesgcm|{key.hex()}|{nonce.hex()}|{hx(aad)}|{hx(msg)}|{AESGCM(key).encrypt(nonce, msg, aad).hex()}")

# ----- X25519 and Ed25519 --------------------------------------------------------------------------------------


def raw_public(pub) -> bytes:
    return pub.public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)


def x25519_ref(scalar: bytes, u: bytes) -> bytes:
    return X25519PrivateKey.from_private_bytes(scalar).exchange(X25519PublicKey.from_public_bytes(u))


# RFC 7748 section 5.2 and 6.1 (published)
s52 = bytes.fromhex("a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4")
u52 = bytes.fromhex("e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c")
assert x25519_ref(s52, u52).hex() == "c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552"
add("x25519/dh.kat", f"rfc7748_5_2|{s52.hex()}|{u52.hex()}|c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552")
alice = bytes.fromhex("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a")
bob = bytes.fromhex("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb")
alice_pub = raw_public(X25519PrivateKey.from_private_bytes(alice).public_key())
bob_pub = raw_public(X25519PrivateKey.from_private_bytes(bob).public_key())
assert alice_pub.hex() == "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a"
assert bob_pub.hex() == "de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f"
shared = x25519_ref(alice, bob_pub)
assert shared.hex() == "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742"
add("x25519/public.kat", f"rfc7748_6_1_alice|{alice.hex()}|{alice_pub.hex()}")
add("x25519/public.kat", f"rfc7748_6_1_bob|{bob.hex()}|{bob_pub.hex()}")
add("x25519/dh.kat", f"rfc7748_6_1_alice_bob|{alice.hex()}|{bob_pub.hex()}|{shared.hex()}")
add("x25519/dh.kat", f"rfc7748_6_1_bob_alice|{bob.hex()}|{alice_pub.hex()}|{shared.hex()}")
for sd in (1, 50, 100, 200):
    a, b = pattern(32, sd), pattern(32, sd + 7)
    pb = raw_public(X25519PrivateKey.from_private_bytes(b).public_key())
    add("x25519/public.kat", f"derived_{sd}|{a.hex()}|{raw_public(X25519PrivateKey.from_private_bytes(a).public_key()).hex()}")
    add("x25519/dh.kat", f"derived_{sd}|{a.hex()}|{pb.hex()}|{x25519_ref(a, pb).hex()}")
# a u-coordinate with the high bit set is masked (RFC 7748 section 5)
hi_bit = bytearray(u52)
hi_bit[31] |= 0x80
add("x25519/dh.kat", f"high_bit_masked|{s52.hex()}|{bytes(hi_bit).hex()}|{x25519_ref(s52, bytes(hi_bit)).hex()}")

# RFC 8032 section 7.1 tests 1 and 2 (published)
ed_cases = [
    ("rfc8032_test1", "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60", b"",
     "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
     "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"),
    ("rfc8032_test2", "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb", bytes.fromhex("72"),
     "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
     "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00"),
]
for label, seed_hex, msg, pub_hex, sig_hex in ed_cases:
    key = Ed25519PrivateKey.from_private_bytes(bytes.fromhex(seed_hex))
    assert raw_public(key.public_key()).hex() == pub_hex
    assert key.sign(msg).hex() == sig_hex
    add("ed25519/sign.kat", f"{label}|{seed_hex}|{pub_hex}|{hx(msg)}|{sig_hex}")
for sd in (1, 50, 100, 200):
    seed = pattern(32, sd)
    key = Ed25519PrivateKey.from_private_bytes(seed)
    for ln in (0, 5, 64, 200):
        msg = pattern(ln, sd + 3)
        add("ed25519/sign.kat", f"derived_{sd}_len{ln}|{seed.hex()}|{raw_public(key.public_key()).hex()}|{hx(msg)}|{key.sign(msg).hex()}")

# ----- write --------------------------------------------------------------------------------------------------

for folder in sorted({p.split("/")[0] for p in files}):
    shutil.rmtree(ROOT / folder, ignore_errors=True)
for path, lines in sorted(files.items()):
    target = ROOT / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text("\n".join(lines) + "\n", newline="\n")
print(f"wrote {sum(len(v) for v in files.values())} vectors in {len(files)} files")
