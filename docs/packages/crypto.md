# crypto

Hashes, message authentication, key derivation, authenticated encryption, and key agreement, written in pure Burdin. The package is an experiment (`spec/crypto.md`): its code is not audited, and it makes no security claim. Use an established library where the stakes are real.

Version `0.1.0`. Use it with `use crypto`; it is an official package, so `burdin.toml` needs no entry for it. The package's own guide is [`packages/crypto/README.md`](../../packages/crypto/README.md).

## Requirements

Nothing beyond the compiler.

## Worked example

Seals a message with ChaCha20-Poly1305, opens it again, and computes a SHA-256 digest and an HMAC.

```burdin
use crypto.{random_bytes, chacha20_poly1305_seal, chacha20_poly1305_open, constant_time_eq, sha256, hmac, hmac_verify, HashAlg}

fn main() {
    let key = random_bytes(32)
    let nonce = random_bytes(12)
    let header = "header".to_utf8()
    let secret = "secret message".to_utf8()
    let sealed = chacha20_poly1305_seal(key.as_slice(), nonce.as_slice(), header.as_slice(), secret.as_slice()).unwrap()
    let opened = chacha20_poly1305_open(key.as_slice(), nonce.as_slice(), header.as_slice(), sealed.as_slice()).unwrap()
    assert(constant_time_eq(opened.as_slice(), secret.as_slice()))

    let digest = sha256(secret.as_slice())
    assert_eq(digest.len(), 32)
    let tag = hmac(HashAlg.Sha256, key.as_slice(), secret.as_slice())
    assert(hmac_verify(HashAlg.Sha256, key.as_slice(), secret.as_slice(), tag.as_slice()))
    println("sealed {} bytes, tag {} bytes", sealed.len(), tag.len())
}
```

Save it as `main.bd` and run it with `burdin run main.bd`, or copy the body into a package that declares `use crypto`. Every example in the package's doc comments is also compiled and run by `burdin test --doc`.

## Surface

Every public item and public method, in source order. Names are shown as `burdin doc` shows them: an item is `module.Name`, and a package whose root re-exports it (`use tls.{Certificate}`) also reaches it from the root. Each row's summary is the first sentence of the item's doc comment; the full text and the examples are on the generated site.

| Item | Signature | Summary |
| --- | --- | --- |
| `aes.Aes` | `struct Aes` | An AES key schedule for a 128-, 192-, or 256-bit key. |
| `aes.Aes.new` | `fn new(key: [u8]): Result<Aes, CryptoError>` | Expands a 16-, 24-, or 32-byte key. |
| `aes.Aes.encrypt_block` | `fn encrypt_block(self, block: [u8]): List<u8>` | Encrypts one 16-byte block. |
| `aes.Aes.decrypt_block` | `fn decrypt_block(self, block: [u8]): List<u8>` | Decrypts one 16-byte block. |
| `aes.aes_ctr_xor` | `fn aes_ctr_xor(key: [u8], iv: [u8], data: [u8]): Result<List<u8>, CryptoError>` | AES-CTR (SP 800-38A): XORs `data` with the keystream of a 16-byte initial counter block, incremented as a big-endian 128-bit number. |
| `aes.aes_cbc_encrypt` | `fn aes_cbc_encrypt(key: [u8], iv: [u8], plaintext: [u8]): Result<List<u8>, CryptoError>` | AES-CBC with PKCS#7 padding. |
| `aes.aes_cbc_decrypt` | `fn aes_cbc_decrypt(key: [u8], iv: [u8], ciphertext: [u8]): Result<List<u8>, CryptoError>` | Decrypts `aes_cbc_encrypt` output and removes the padding; bad padding is `InvalidPadding`. |
| `aes.aes_gcm_seal` | `fn aes_gcm_seal(key: [u8], nonce: [u8], aad: [u8], plaintext: [u8]): Result<List<u8>, CryptoError>` | AES-GCM (SP 800-38D) with a 12-byte nonce and a 16-byte tag: encrypts `plaintext` and authenticates it together with `aad`. |
| `aes.aes_gcm_open` | `fn aes_gcm_open(key: [u8], nonce: [u8], aad: [u8], sealed: [u8]): Result<List<u8>, CryptoError>` | Verifies and decrypts `aes_gcm_seal` output; a wrong tag is `AuthenticationFailed` and no plaintext is released. |
| `chacha.chacha20_block` | `fn chacha20_block(key: [u8], counter: u32, nonce: [u8]): List<u8>` | One 64-byte ChaCha20 keystream block (`key` is 32 bytes, `nonce` 12 bytes). |
| `chacha.chacha20_xor` | `fn chacha20_xor(key: [u8], nonce: [u8], counter: u32, data: [u8]): Result<List<u8>, CryptoError>` | Encrypts or decrypts `data` with ChaCha20 (RFC 8439): XOR with the keystream starting at block `counter`. |
| `chacha.hchacha20` | `fn hchacha20(key: [u8], nonce: [u8]): Result<List<u8>, CryptoError>` | HChaCha20: a 32-byte subkey from a 32-byte key and a 16-byte nonce (the first step of XChaCha20). |
| `chacha.xchacha20_xor` | `fn xchacha20_xor(key: [u8], nonce: [u8], counter: u32, data: [u8]): Result<List<u8>, CryptoError>` | XChaCha20 (24-byte nonce): encrypts or decrypts `data`, starting at block `counter`. |
| `chacha.Poly1305` | `struct Poly1305` | An incremental Poly1305 one-time authenticator (RFC 8439). |
| `chacha.Poly1305.new` | `fn new(key: [u8]): Result<Poly1305, CryptoError>` | A new authenticator for a 32-byte one-time key. |
| `chacha.Poly1305.update` | `fn update(mut self, data: [u8])` | Authenticates more of the message. |
| `chacha.Poly1305.finish` | `fn finish(mut self): List<u8>` | The 16-byte tag. |
| `chacha.poly1305` | `fn poly1305(key: [u8], data: [u8]): Result<List<u8>, CryptoError>` | The Poly1305 tag of `data` under a 32-byte one-time `key`. |
| `chacha.chacha20_poly1305_seal` | `fn chacha20_poly1305_seal(key: [u8], nonce: [u8], aad: [u8], plaintext: [u8]): Result<List<u8>, CryptoError>` | ChaCha20-Poly1305 (RFC 8439): encrypts `plaintext` under a 32-byte `key` and 12-byte `nonce` and authenticates it with `aad`. |
| `chacha.chacha20_poly1305_open` | `fn chacha20_poly1305_open(key: [u8], nonce: [u8], aad: [u8], sealed: [u8]): Result<List<u8>, CryptoError>` | Verifies and decrypts the output of `chacha20_poly1305_seal`; a wrong tag is `AuthenticationFailed` and no plaintext is released. |
| `chacha.xchacha20_poly1305_seal` | `fn xchacha20_poly1305_seal(key: [u8], nonce: [u8], aad: [u8], plaintext: [u8]): Result<List<u8>, CryptoError>` | XChaCha20-Poly1305: the same AEAD with a 24-byte nonce, which is safe to pick at random. |
| `chacha.xchacha20_poly1305_open` | `fn xchacha20_poly1305_open(key: [u8], nonce: [u8], aad: [u8], sealed: [u8]): Result<List<u8>, CryptoError>` | Verifies and decrypts the output of `xchacha20_poly1305_seal`. |
| `curve25519.x25519` | `fn x25519(scalar: [u8], point: [u8]): Result<List<u8>, CryptoError>` | X25519 (RFC 7748): the Diffie-Hellman function on Curve25519. |
| `curve25519.x25519_public_key` | `fn x25519_public_key(scalar: [u8]): Result<List<u8>, CryptoError>` | The X25519 public key for a 32-byte secret: the scalar times the base point (u = 9). |
| `curve25519.x25519_generate_secret` | `fn x25519_generate_secret(): List<u8>` | A new random X25519 secret key (32 bytes from the OS entropy source). |
| `curve25519.ed25519_public_key` | `fn ed25519_public_key(seed: [u8]): Result<List<u8>, CryptoError>` | The Ed25519 public key (32 bytes) of a 32-byte secret seed. |
| `curve25519.ed25519_generate` | `fn ed25519_generate(): (List<u8>, List<u8>)` | A new random Ed25519 key pair: the 32-byte secret seed and the 32-byte public key. |
| `curve25519.ed25519_sign` | `fn ed25519_sign(seed: [u8], message: [u8]): Result<List<u8>, CryptoError>` | Signs `message` with a 32-byte secret seed; the signature is 64 bytes (R, then S). |
| `curve25519.ed25519_verify` | `fn ed25519_verify(public_key: [u8], message: [u8], signature: [u8]): bool` | Whether `signature` is a valid Ed25519 signature of `message` under `public_key`. |
| `errors.CryptoError` | `error enum CryptoError: Eq, Debug` | The error every fallible function of this package reports. |
| `hash.HashAlg` | `enum HashAlg: Copy, Clone, Eq, Debug` | A hash function. |
| `hash.HashAlg.block_len` | `fn block_len(self): usize` | The input block size in bytes (the rate, for SHA-3). |
| `hash.HashAlg.output_len` | `fn output_len(self): usize` | The digest size in bytes. |
| `hash.HashAlg.name` | `fn name(self): str` | The name, as in `"SHA-256"`. |
| `hash.HashAlg.digest` | `fn digest(self, data: [u8]): List<u8>` | The digest of `data`. |
| `hmac.hmac` | `fn hmac(alg: HashAlg, key: [u8], data: [u8]): List<u8>` | The HMAC of `data` under `key`. |
| `hmac.hmac_verify` | `fn hmac_verify(alg: HashAlg, key: [u8], data: [u8], tag: [u8]): bool` | Whether `tag` is the HMAC of `data` under `key`, compared in constant time. |
| `hmac.Hmac` | `struct Hmac` | An incremental HMAC. |
| `hmac.Hmac.new` | `fn new(alg: HashAlg, key: [u8]): Hmac` | A new HMAC over `alg` with `key`. |
| `hmac.Hmac.update` | `fn update(mut self, data: [u8])` | Adds more of the message. |
| `hmac.Hmac.finish` | `fn finish(mut self): List<u8>` | The tag. |
| `hmac.hkdf_extract` | `fn hkdf_extract(alg: HashAlg, salt: [u8], ikm: [u8]): List<u8>` | HKDF-Extract: a pseudorandom key from input keying material. |
| `hmac.hkdf_expand` | `fn hkdf_expand(alg: HashAlg, prk: [u8], info: [u8], length: usize): Result<List<u8>, CryptoError>` | HKDF-Expand: `length` bytes of output keying material from a pseudorandom key. |
| `hmac.hkdf` | `fn hkdf(alg: HashAlg, salt: [u8], ikm: [u8], info: [u8], length: usize): Result<List<u8>, CryptoError>` | HKDF (extract, then expand). |
| `hmac.pbkdf2` | `fn pbkdf2(alg: HashAlg, password: [u8], salt: [u8], iterations: u32, length: usize): Result<List<u8>, CryptoError>` | PBKDF2: `length` bytes derived from `password` and `salt` with `iterations` rounds of HMAC. |
| `md5.Md5` | `struct Md5` | An incremental MD5 hasher (insecure; see the file comment). |
| `md5.Md5.new` | `fn new(): Md5` | A new MD5 hasher, which produces a 16-byte digest. |
| `md5.Md5.block_len` | `fn block_len(self): usize` | The block size in bytes (64). |
| `md5.Md5.output_len` | `fn output_len(self): usize` | The digest size in bytes (16). |
| `md5.Md5.update` | `fn update(mut self, data: [u8])` | Hashes more input. |
| `md5.Md5.finish` | `fn finish(mut self): List<u8>` | Ends the hash and returns the digest. |
| `md5.md5` | `fn md5(data: [u8]): List<u8>` | The MD5 digest of `data` (insecure; see the file comment). |
| `sha1.Sha1` | `struct Sha1` | An incremental SHA-1 hasher (insecure for collisions; see the file comment). |
| `sha1.Sha1.new` | `fn new(): Sha1` | A new SHA-1 hasher, which produces a 20-byte digest. |
| `sha1.Sha1.block_len` | `fn block_len(self): usize` | The block size in bytes (64). |
| `sha1.Sha1.output_len` | `fn output_len(self): usize` | The digest size in bytes (20). |
| `sha1.Sha1.update` | `fn update(mut self, data: [u8])` | Hashes more input. |
| `sha1.Sha1.finish` | `fn finish(mut self): List<u8>` | Ends the hash and returns the digest. |
| `sha1.sha1` | `fn sha1(data: [u8]): List<u8>` | The SHA-1 digest of `data` (insecure for collisions; see the file comment). |
| `sha256.Sha256` | `struct Sha256` | An incremental SHA-256 (or SHA-224) hasher. |
| `sha256.Sha256.new` | `fn new(): Sha256` | A SHA-256 hasher (32-byte digest). |
| `sha256.Sha256.new224` | `fn new224(): Sha256` | A SHA-224 hasher (28-byte digest). |
| `sha256.Sha256.block_len` | `fn block_len(self): usize` | The block size in bytes (64). |
| `sha256.Sha256.output_len` | `fn output_len(self): usize` | The digest size in bytes. |
| `sha256.Sha256.update` | `fn update(mut self, data: [u8])` | Hashes more input. |
| `sha256.Sha256.finish` | `fn finish(mut self): List<u8>` | Ends the hash and returns the digest. |
| `sha256.sha256` | `fn sha256(data: [u8]): List<u8>` | The SHA-256 digest of `data`. |
| `sha256.sha224` | `fn sha224(data: [u8]): List<u8>` | The SHA-224 digest of `data`. |
| `sha3.Sha3` | `struct Sha3` | An incremental SHA-3 hasher, or a SHAKE extendable-output function. |
| `sha3.Sha3.new224` | `fn new224(): Sha3` | SHA3-224 (28-byte digest). |
| `sha3.Sha3.new256` | `fn new256(): Sha3` | SHA3-256 (32-byte digest). |
| `sha3.Sha3.new384` | `fn new384(): Sha3` | SHA3-384 (48-byte digest). |
| `sha3.Sha3.new512` | `fn new512(): Sha3` | SHA3-512 (64-byte digest). |
| `sha3.Sha3.shake128` | `fn shake128(out_len: usize): Sha3` | SHAKE128 producing `out_len` bytes. |
| `sha3.Sha3.shake256` | `fn shake256(out_len: usize): Sha3` | SHAKE256 producing `out_len` bytes. |
| `sha3.Sha3.block_len` | `fn block_len(self): usize` | The block size in bytes (the sponge rate). |
| `sha3.Sha3.output_len` | `fn output_len(self): usize` | The digest size in bytes. |
| `sha3.Sha3.update` | `fn update(mut self, data: [u8])` | Hashes more input. |
| `sha3.Sha3.finish` | `fn finish(mut self): List<u8>` | Ends the hash and returns the digest. |
| `sha3.sha3_224` | `fn sha3_224(data: [u8]): List<u8>` | The SHA3-224 digest of `data`. |
| `sha3.sha3_256` | `fn sha3_256(data: [u8]): List<u8>` | The SHA3-256 digest of `data`. |
| `sha3.sha3_384` | `fn sha3_384(data: [u8]): List<u8>` | The SHA3-384 digest of `data`. |
| `sha3.sha3_512` | `fn sha3_512(data: [u8]): List<u8>` | The SHA3-512 digest of `data`. |
| `sha3.shake128` | `fn shake128(data: [u8], out_len: usize): List<u8>` | `out_len` bytes of SHAKE128 output for `data`. |
| `sha3.shake256` | `fn shake256(data: [u8], out_len: usize): List<u8>` | `out_len` bytes of SHAKE256 output for `data`. |
| `sha512.Sha512` | `struct Sha512` | An incremental SHA-512 (or SHA-384) hasher. |
| `sha512.Sha512.new` | `fn new(): Sha512` | A SHA-512 hasher (64-byte digest). |
| `sha512.Sha512.new384` | `fn new384(): Sha512` | A SHA-384 hasher (48-byte digest). |
| `sha512.Sha512.block_len` | `fn block_len(self): usize` | The block size in bytes (128). |
| `sha512.Sha512.output_len` | `fn output_len(self): usize` | The digest size in bytes. |
| `sha512.Sha512.update` | `fn update(mut self, data: [u8])` | Hashes more input. |
| `sha512.Sha512.finish` | `fn finish(mut self): List<u8>` | Ends the hash and returns the digest. |
| `sha512.sha512` | `fn sha512(data: [u8]): List<u8>` | The SHA-512 digest of `data`. |
| `sha512.sha384` | `fn sha384(data: [u8]): List<u8>` | The SHA-384 digest of `data`. |
| `util.constant_time_eq` | `fn constant_time_eq(a: [u8], b: [u8]): bool` | Compares two byte strings without stopping at the first difference, so the time does not reveal where they differ. |
| `util.wipe` | `fn wipe(mut buf: [u8])` | Overwrites `buf` with zeros. |
| `util.xor_bytes` | `fn xor_bytes(a: [u8], b: [u8]): List<u8>` | The bytes of `a` XORed with the bytes of `b`; the shorter length wins. |
| `util.random_bytes` | `fn random_bytes(count: usize): List<u8>` | `count` random bytes from the OS entropy source (`getentropy` on Unix, `rand_s` on Windows), suitable for keys, nonces, and salts. |
