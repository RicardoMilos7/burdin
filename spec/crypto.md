# Crypto

The cryptography package (stage 3, issue #266; `packages/crypto`). It is an ordinary official package
(`spec/packages.md`, PKG15-PKG18) written entirely in Burdin, so this page says which algorithms it provides, what each
function accepts and reports, and which tests witness each rule. The rule IDs are CR1-CR14. The package documentation,
`packages/crypto/README.md`, repeats the decisions for users.

**This is an experiment, not a security claim.** The package is checked against published vectors and against OpenSSL.
That shows it computes the right answers; it does not show that it resists a side channel, a fault, or a mistake in
how a program uses it. It has had no audit. (CR14)

Failures are values: a `CryptoError` in a `Result`, or `false` from a verifier, never a panic and never a compiler
diagnostic, so no registry code is involved (`tests/README.md`). The accepted behavior is pinned by the known-answer
fixtures under `tests/crypto/` and by the conformance case `tests/s3_crypto/usage`.

## CR1. Native, not a C library (decision)

Every algorithm is Burdin. The issue asked to decide native versus a C library, and this is the decision: no system
library to find or link on any platform, no `unsafe`, no FFI, code that the checker and the test runner see, and an
independent oracle (OpenSSL through Python) to test against. The costs are speed and the absence of hardware
instructions. A program that needs them binds OpenSSL or libsodium with `burdin bindgen` (`spec/bindgen.md`).
Witnessed by `packages/crypto` having no `extern` declaration.

## CR2. Types

Inputs are `[u8]` views and outputs are `List<u8>`; a fixed-size output (a digest, a tag, a key) is a list of exactly
that many bytes. Every fallible function returns `Result<T, CryptoError>`. `CryptoError` is an error enum with the
variants `InvalidKeyLength { expected, got }`, `InvalidNonceLength { expected, got }`, `InvalidLength { what }`,
`InvalidPadding`, `InvalidInput { what }`, `AuthenticationFailed`, `InvalidSignature`, and `InvalidPoint`; its
messages are pinned by `tests/crypto/errors/scenarios.kat`.

## CR3. Hashes

`sha256`, `sha224`, `sha512`, `sha384` (FIPS 180-4), `sha3_224`, `sha3_256`, `sha3_384`, `sha3_512`, `shake128`, and
`shake256` (FIPS 202), plus `sha1` and `md5`. Every hash has a one-shot function and an incremental hasher (`Sha256`,
`Sha512`, `Sha3`, `Sha1`, `Md5`) with `update(data)` and `finish()`, and feeding a message in any chunking gives the
digest of the whole message. `finish` ends the hasher. `md5` and `sha1` are documented as broken for collision
resistance and exist only for interoperability. `HashAlg` names the ten fixed-output hashes and offers `digest`,
`block_len`, `output_len`, and `name`. Witnessed by `tests/crypto/hash/*.kat` (the empty message, `abc`, the FIPS two-block
message, every length around each block and rate boundary, one million `a`, and one-byte and seven-byte chunking).

## CR4. HMAC

`hmac(alg, key, data)` is RFC 2104 over any `HashAlg`: a key longer than the block is hashed first, a shorter key is
padded with zeros. `hmac_verify` compares in constant time. `Hmac` is the incremental form. Witnessed by
`tests/crypto/hmac/*.kat` (RFC 4231 cases 1-4, 6, and 7, empty and block-sized keys, for all ten hashes).

## CR5. HKDF and PBKDF2

`hkdf_extract`, `hkdf_expand`, and `hkdf` are RFC 5869; an empty salt is a string of zeros as long as the digest;
`hkdf_expand` with a length above 255 times the digest size is `InvalidLength`, and exactly that maximum succeeds.
`pbkdf2` is RFC 8018 with HMAC; zero iterations is `InvalidInput`. Witnessed by `tests/crypto/hkdf/*.kat` (RFC 5869
cases 1 and 3), `tests/crypto/pbkdf2/*.kat` (RFC 6070), and the `hkdf_too_long`, `hkdf_longest`, and `pbkdf2_zero`
scenarios.

## CR6. ChaCha20 and Poly1305

`chacha20_xor` is RFC 8439 (a 32-byte key, a 12-byte nonce, a 32-bit starting block counter), `hchacha20` and
`xchacha20_xor` extend the nonce to 24 bytes, and `Poly1305` / `poly1305` is the one-time authenticator (a 32-byte key
that must never authenticate two messages). A wrong key or nonce length is `InvalidKeyLength` or
`InvalidNonceLength`. Witnessed by `tests/crypto/chacha20/*.kat`, `tests/crypto/poly1305/*.kat` (RFC 8439 sections
2.4.2 and 2.5.2, the XChaCha draft's HChaCha20 vector, and every length around the 16-byte block).

## CR7. Authenticated encryption

`chacha20_poly1305_seal` / `_open` (RFC 8439 section 2.8), `xchacha20_poly1305_seal` / `_open` (a 24-byte nonce), and
`aes_gcm_seal` / `_open` (a 12-byte nonce, a 16-byte tag, a 16-, 24-, or 32-byte key). `seal` returns the ciphertext
followed by the tag. `open` checks the tag in constant time **before** it decrypts, returns `AuthenticationFailed` for
any change to the ciphertext, the tag, the nonce, or the associated data, and `InvalidLength` for input shorter than a
tag, and releases no plaintext on failure. Witnessed by `tests/crypto/aead/*.kat` (RFC 8439 section 2.8.2, the GCM
specification's test cases 1 and 2, and every block boundary), where the runner also flips a ciphertext bit, a tag bit,
and the associated data, and truncates the message, for every vector.

## CR8. AES

`Aes` is FIPS 197 for 128-, 192-, and 256-bit keys with `encrypt_block` and `decrypt_block`; any other key length is
`InvalidLength`. `aes_ctr_xor` is SP 800-38A counter mode with a 16-byte initial counter block incremented as a
big-endian 128-bit number. `aes_cbc_encrypt` and `aes_cbc_decrypt` are CBC with PKCS#7 padding: decryption of a length
that is not a non-empty multiple of 16 is `InvalidLength` and bad padding is `InvalidPadding`. CTR and CBC do not
authenticate, and the documentation says to use GCM or to MAC the ciphertext. There is no ECB mode beyond the single
block. Witnessed by `tests/crypto/aes/*.kat` (FIPS 197 appendix C, SP 800-38A F.2.1 and F.5.1, all three key sizes,
and a counter that carries out of its low byte).

## CR9. X25519

`x25519(secret, point)` is RFC 7748 (the secret is clamped, the top bit of the point is ignored). An all-zero output,
which a low-order peer point produces, is `InvalidPoint`. `x25519_public_key` multiplies the base point (u = 9), and
`x25519_generate_secret` is 32 random bytes. Witnessed by `tests/crypto/x25519/*.kat` (RFC 7748 sections 5.2 and 6.1,
and a point with the high bit set) and the `x25519_*` scenarios.

## CR10. Ed25519

`ed25519_public_key(seed)`, `ed25519_sign(seed, message)` (64 bytes, deterministic), and
`ed25519_verify(public_key, message, signature)` are RFC 8032 (the pure variant, no context, no prehash). `verify`
returns `false`, not an error, for a wrong length, a public key that is not on the curve, and a scalar at or above the
group order (so a signature has one encoding). Witnessed by `tests/crypto/ed25519/sign.kat` (RFC 8032 section 7.1 tests
1 and 2), where the runner also checks a changed signature, a changed message, and a non-canonical scalar, and by the
`ed25519_*` scenarios.

## CR11. Random bytes

`random_bytes(count)` returns `count` bytes taken from the operating system's entropy source, 64 bits at a time,
through `random_seed()` (`spec/stdlib.md`). It keeps no state, so there is nothing to seed. It is not available at
compile time (`random_seed` is E0224 there). Witnessed by `tests/s3_crypto/usage` (the length only: random output has
no fixed answer).

## CR12. Constant time

The ChaCha20, Poly1305, X25519, Ed25519, and GHASH arithmetic uses fixed-width operations and masks, with no branch and
no table index that depends on a secret; tag comparison is `constant_time_eq`. The AES S-box is indexed by secret data
and is **not** constant time against a cache-timing attacker, and the documentation says so. The hashes branch only on
the message length. These are properties of the source; Burdin does not promise that the compiler preserves them, and
nothing here tests timing.

## CR13. No panics

Integer arithmetic that can wrap uses `wrapping_*` and fixed-width masks, and every length is checked before it is
used, so no input makes a function panic. Witnessed by the error scenarios, and by the runner decoding and verifying
corrupted input for every vector without a crash.

## CR14. Not a security claim

See the top of this page. The README repeats it, and `tests/check_crypto.py` fails if the README stops saying so.

## Tests

| Rule | Tests |
| --- | --- |
| CR1-CR14 | `tests/crypto/` (about 500 known-answer vectors and the error scenarios), run by `tests/check_crypto.py` through `tests/crypto/runner`; CI runs the check |
| CR2-CR11 | the conformance case `tests/s3_crypto/usage`, run by `tests/run_tests.py` |
| provenance | `tests/crypto/regenerate.py`: published vectors asserted against OpenSSL before any fixture is written |
