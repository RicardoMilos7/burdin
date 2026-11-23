# crypto

Hashing and the common cryptographic algorithms, written in pure Burdin (issue #266; the behavior is
`spec/crypto.md`). Use it with `use crypto`; no `burdin.toml` entry is needed.

```burdin
use crypto.{sha256, hmac, HashAlg, chacha20_poly1305_seal, chacha20_poly1305_open, random_bytes}

let digest = sha256("hello".as_bytes())                       // List<u8>
println("{}", digest.to_hex())

let key = random_bytes(32)
let nonce = random_bytes(12)
let sealed = chacha20_poly1305_seal(key.as_slice(), nonce.as_slice(), "header".as_bytes(), "secret".as_bytes())?
let plain = chacha20_poly1305_open(key.as_slice(), nonce.as_slice(), "header".as_bytes(), sealed.as_slice())?
```

> **This is an experiment, not a security claim.** Burdin is a personal, AI-assisted language experiment. This
> package has had no audit, no formal verification, and no side-channel testing on real hardware. It is checked
> against published test vectors and against OpenSSL, which proves it computes the right answers, not that it is safe
> to protect anything you care about. Use a reviewed library for real secrets.

## Decision: native, not a C library

The issue asked to decide native versus a C library. Every algorithm here is written in Burdin. Reasons: the package
then works on every platform the compiler does with no system library to find or link (OpenSSL is not on Windows
by default); it needs no `unsafe` and no FFI; the code is ordinary Burdin that the checker, the formatter, and the
test runner see; and a known-answer test against OpenSSL (through Python, in `tests/crypto/regenerate.py`) gives an
independent oracle. The price is speed (see Performance) and the absence of hardware instructions (AES-NI, SHA
extensions). A program that needs those can bind OpenSSL or libsodium through `burdin bindgen`.

## Surface

Byte inputs are `[u8]` views (`list.as_slice()`, `"text".as_bytes()`); byte outputs are `List<u8>`. `List<u8>` has
`to_hex()` and `to_base64()`, and `hex_decode` / `base64_decode` are in the standard library. A function that can fail
returns `Result<T, CryptoError>`.

### Hashes

```
sha256, sha224, sha512, sha384, sha3_224, sha3_256, sha3_384, sha3_512, sha1, md5   fn(data: [u8]): List<u8>
shake128, shake256                                                                   fn(data: [u8], out_len: usize): List<u8>

Sha256.new() / Sha256.new224() / Sha512.new() / Sha512.new384() / Sha3.new256() ...  an incremental hasher
hasher.update(data: [u8])        hasher.finish(): List<u8>        hasher.block_len()      hasher.output_len()

enum HashAlg { Md5, Sha1, Sha224, Sha256, Sha384, Sha512, Sha3_224, Sha3_256, Sha3_384, Sha3_512 }
alg.digest(data)    alg.block_len()    alg.output_len()    alg.name()
```

`md5` and `sha1` are **broken for collision resistance**. They are here for old protocols and formats (Git, legacy
HMAC-SHA1) and are named so in the code; do not use them to fingerprint input an attacker controls. A hasher must not
be used after `finish`.

### MACs and key derivation

```
hmac(alg, key, data): List<u8>                  hmac_verify(alg, key, data, tag): bool     // constant-time compare
Hmac.new(alg, key)   .update(data)   .finish()  // incremental
hkdf(alg, salt, ikm, info, length): Result<List<u8>, CryptoError>      // also hkdf_extract and hkdf_expand
pbkdf2(alg, password, salt, iterations, length): Result<List<u8>, CryptoError>
```

`pbkdf2` is a password-stretching function, not a memory-hard one (there is no scrypt or Argon2 here). Use at least
600,000 iterations of `HashAlg.Sha256` for a new password hash; a debug build takes about ten times
as long, so build with `--release`.

### Ciphers and authenticated encryption

```
chacha20_poly1305_seal(key, nonce, aad, plaintext): Result<List<u8>, CryptoError>    // ciphertext || 16-byte tag
chacha20_poly1305_open(key, nonce, aad, sealed):    Result<List<u8>, CryptoError>
xchacha20_poly1305_seal / _open                                                       // 24-byte nonce
aes_gcm_seal(key, nonce, aad, plaintext) / aes_gcm_open(key, nonce, aad, sealed)      // 16/24/32-byte key, 12-byte nonce
chacha20_xor(key, nonce, counter, data)      xchacha20_xor(...)      hchacha20(key, nonce16)
aes_ctr_xor(key, iv, data)                   aes_cbc_encrypt(key, iv, plaintext)      aes_cbc_decrypt(key, iv, ciphertext)
Aes.new(key)  .encrypt_block(block)  .decrypt_block(block)                            // one 16-byte block
Poly1305.new(key)  .update(data)  .finish()      poly1305(key, data)
```

Prefer the AEADs (`*_seal` / `*_open`): they encrypt and authenticate together, and `open` returns
`CryptoError.AuthenticationFailed` and releases no plaintext when anything was changed. Never reuse a (key, nonce)
pair with ChaCha20-Poly1305 or AES-GCM. A random 24-byte nonce is safe with XChaCha20-Poly1305; a random 12-byte
nonce is only safe for about 2^32 messages per key. `aes_ctr_xor` and `aes_cbc_*` do **not** authenticate; CBC
decryption that reveals padding errors is a padding oracle, so MAC the ciphertext (encrypt-then-MAC) or use GCM.
There is no ECB mode beyond the single-block `Aes` methods.

### Key agreement and signatures

```
x25519(secret, peer_public): Result<List<u8>, CryptoError>       x25519_public_key(secret)     x25519_generate_secret()
ed25519_public_key(seed): Result<List<u8>, CryptoError>          ed25519_sign(seed, message)   // 64 bytes
ed25519_verify(public_key, message, signature): bool             ed25519_generate(): (seed, public_key)
```

`x25519` reports an all-zero result (the peer sent a low-order point) as `CryptoError.InvalidPoint`; pass the shared
secret through `hkdf` before using it as a key. `ed25519_verify` returns `false` for anything malformed (a wrong
length, a point off the curve, a scalar at or above the group order) and never panics. Ed25519 signing is
deterministic.

### Helpers

```
constant_time_eq(a, b): bool        wipe(mut buf)        xor_bytes(a, b)        random_bytes(count): List<u8>
```

`random_bytes` takes 64 bits at a time from the operating system (`getentropy` on Unix, `rand_s` on Windows) through
`random_seed`; there is no user-space generator to seed badly.

## Constant time, and what is not

* **Constant time in the secret:** the ChaCha20, Poly1305, X25519, and Ed25519 arithmetic (add, rotate, xor, and
  multiplications of fixed width, with mask-based selection), the GHASH multiplication in GCM, `constant_time_eq`, and
  tag comparison in every `open`.
* **Not constant time:** AES looks up the S-box with a secret-dependent index, so a local attacker who shares the CPU
  cache can recover a key (the classic cache-timing attack). The package says so in the code; use ChaCha20-Poly1305
  where that attacker matters. The hashes branch only on the message length, which is not secret in any use here.
* **The compiler may not preserve it.** Constant-time code in a language whose optimizer is free to reorder is a
  best effort, and `wipe` can be optimized away. Nothing in Burdin promises either.
* **Panics.** Wrong lengths, bad tags, and malformed signatures are `Err` or `false`, never panics. Integer overflow
  would panic in a checked build, so the arithmetic uses `wrapping_*` throughout.

## Performance

Measured on one machine (WSL on an ordinary x86-64 desktop; yours will differ), release build, one thread:

| operation | time |
| --- | --- |
| SHA-256 of 1 MB | 7.6 ms (about 130 MB/s) |
| SHA-512 of 1 MB | 14 ms |
| ChaCha20-Poly1305 of 100 KB | 3.3 ms |
| AES-GCM of 100 KB | 13 ms |
| X25519 | 0.8 ms |
| Ed25519 sign / verify | 3.0 ms / 2.6 ms |
| PBKDF2-HMAC-SHA-256, 10,000 iterations | 33 ms |

A debug build is 5 to 15 times slower. This is well below OpenSSL, which uses hardware instructions (SHA extensions,
AES-NI, vectorized ChaCha20). A new password hash at the recommended 600,000 PBKDF2 rounds takes about two seconds in
a release build, so run it off the request path. The package favors being short and readable over speed.

## Not here

RSA, ECDSA, P-256, Argon2, scrypt, TLS, X.509, and any 128-bit-nonce-misuse-resistant mode (AES-GCM-SIV). TLS and
auth protocols build on this package and belong in a later one.

## Tests

`tests/crypto/` holds about 500 known-answer vectors, each either published (RFC 4231, 5869, 6070, 7748, 8032, 8439,
FIPS 180, 197, and 202, NIST SP 800-38A and 800-38D) or produced by OpenSSL; `tests/crypto/regenerate.py` writes them
and asserts the published ones. `tests/check_crypto.py` runs them through the package, and `tests/s3_crypto/usage` is
the conformance case.
