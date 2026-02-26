# Encryption Test Vectors

**Purpose**: Cross-implementation validation for cordelia-core (Rust) and cordelia-proxy (TypeScript).
**Generated with**: libsodium via PyNaCl 1.6.2 (authoritative implementation).
**Reference**: [encryption-specification.md](encryption-specification.md)

All hex values are lowercase, little-endian (standard Ed25519/X25519 wire format).

---

## 1. Ed25519 -> X25519 Key Derivation

### Test Vector 1 (RFC 8032 Test Vector 1 seed)

**Source**: RFC 8032 Section 7.1 (Ed25519 seed), libsodium conversion functions.

```
Ed25519 seed (32 bytes):
  9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60

Ed25519 public key (32 bytes):
  d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a

SHA-512(seed) full (64 bytes):
  357c83864f2833cb427a2ef1c00a013cfdff2768d980c0a3a520f006904de90f
  9b4f0afe280b746a778684e75442502057b7473a03f08f96f5a38e9287e01f8f

SHA-512(seed) left half (32 bytes, before clamping):
  357c83864f2833cb427a2ef1c00a013cfdff2768d980c0a3a520f006904de90f

X25519 private key (32 bytes, after clamping):
  307c83864f2833cb427a2ef1c00a013cfdff2768d980c0a3a520f006904de94f

X25519 public key (32 bytes):
  d85e07ec22b0ad881537c2f44d662d1a143cf830c57aca4305d85c7a90f6b62e
```

**Verification**: `crypto_scalarmult_base(x25519_private_key) == x25519_public_key`.

### Test Vector 2 (all-zeros seed)

**Source**: Degenerate case, libsodium conversion functions.

```
Ed25519 seed (32 bytes):
  0000000000000000000000000000000000000000000000000000000000000000

Ed25519 public key (32 bytes):
  3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29

X25519 private key (32 bytes):
  5046adc1dba838867b2bbbfdd0c3423e58b57970b5267a90f57960924a87f156

X25519 public key (32 bytes):
  5bf55c73b82ebe22be80f3430667af570fae2556a6415e6b30d4065300aa947d
```

### Test Vector 3 (libsodium ed25519_convert.c seed)

**Source**: libsodium `test/default/ed25519_convert.c` and `.exp` expected output.

```
Ed25519 seed (32 bytes):
  421151a459faeade3d247115f94aedae42318124095afabe4d1451a559faedee

Ed25519 public key (32 bytes):
  b5076a8474a832daee4dd5b4040983b6623b5f344aca57d4d6ee4baf3f259e6e

X25519 private key (32 bytes):
  8052030376d47112be7f73ed7a019293dd12ad910b654455798b4667d73de166

X25519 public key (32 bytes):
  f1814f0e8ff1043d8a44d25babff3cedcae6c22c3edaa48f857ae70de2baae50
```

### Test Vector 4 (ed2curve-js cross-verified)

**Source**: github.com/dchest/ed2curve-js Issue #4, cross-verified against libsodium.

```
Ed25519 seed (32 bytes):
  9fc9b77445f8b077c29fe27fc581c52beb668ecd25f5bb2ba5777dee2a411e97

Ed25519 public key (32 bytes):
  8fbe438aab6c40dc2ebc839ba27530ca1bf23d4efd36958a3365406efe52ccd1

X25519 private key (32 bytes):
  28e9e1d48cb0e52e437080e4a180058d7a42a07abcd05ea2ec4e6122cded8f6a

X25519 public key (32 bytes):
  26100e941bdd2103038d8dec9a1884694736f591ee814e66ae6e2e2284757136
```

### Derivation Algorithm

The X25519 private key is derived from the Ed25519 seed:

1. Compute `h = SHA-512(ed25519_seed)` (64 bytes)
2. Take `scalar = h[0..32]` (first 32 bytes)
3. Clamp: `scalar[0] &= 0xF8; scalar[31] &= 0x7F; scalar[31] |= 0x40`
4. Result is the X25519 private key

The X25519 public key is converted from the Ed25519 public key (Edwards -> Montgomery point conversion):

- `u = (1 + y) / (1 - y) mod p` where `y` is the Edwards y-coordinate (little-endian decoded from the 32-byte Ed25519 public key, with the top bit cleared)

This is `crypto_sign_ed25519_pk_to_curve25519` in libsodium, `edwardsToMontgomeryPub()` in `@noble/curves`, and the birational equivalence from RFC 7748 Section 4.1.

### Invalid Public Key Test Cases (Expected to Fail)

These Ed25519 public keys MUST be rejected by `pk_to_curve25519`:

```
0000000000000000000000000000000000000000000000000000000000000000  (identity point)
0200000000000000000000000000000000000000000000000000000000000000  (small order)
0500000000000000000000000000000000000000000000000000000000000000  (small order)
```

---

## 2. X25519 ECDH Shared Secret

### Test Vector (IETF Hackathon / OSCORE)

**Source**: IETF hackathon test vectors, github.com/pyca/cryptography Issue #5557, cross-verified against libsodium.

```
Party A:
  Ed25519 seed:      397ceb5a8d21d74a9258c20c33fc45ab152b02cf479b2e3081285f77454cf347
  Ed25519 public key:ce616f28426ef24edb51dbcef7a23305f886f657959d4df889ddfc0255042159
  X25519 private key:48cb217ef470512fd65aba03f501d3d31a91aaed3f32c053caf9b69e26ffbb4c
  X25519 public key: 243cc5b065ea4a4c0bce1264de6a2f3e5c0a578fb1ecb08b0aab6bc90e1cf318

Party B:
  Ed25519 seed:      70559b9eecdc578d5fc2ca37f9969630029f1592aff3306392ab15546c6a184a
  Ed25519 public key:2668ba6ca302f14e952228da1250a890c143fdba4daed27246188b9e42c94b6d
  X25519 private key:b810ae25c57c5c8990af4dff36e3bfec7f614cd294eee2eca9bce76763aaf977
  X25519 public key: fb5346cd1c5726bc11e27586e066d079ed28a2f9db70a54f4b924642424d116b

X25519 shared secret (A_sk * B_pk == B_sk * A_pk):
  4546babdb9482396c167af11d21953bfa49eb9f630c45de93ee4d3b9ef059576
```

---

## 3. HKDF-SHA256 (Envelope Key Derivation)

Used in ECIES envelope encryption to derive the AES-256-GCM wrapping key from the X25519 shared secret.

### Test Vector (Standalone)

```
Shared secret (32 bytes):
  4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742

Salt:
  (empty -- HKDF uses 32 zero bytes when salt is empty)

Info string:
  "cordelia-key-wrap-v1" (UTF-8, 20 bytes, hex: 636f7264656c69612d6b65792d777261702d7631)

PRK (HMAC-SHA256 extract):
  fd5b81e5379cfe3599ea059945250eabdb0913b8c3ae8bd85d6b735f15538be5

OKM / Wrapping key (32 bytes, HKDF expand with counter 0x01):
  f1f4ea6c1d40b1c6a968574803e9e21173846d7b184d522223e8a42705124f9a
```

### HKDF Algorithm

1. **Extract**: `PRK = HMAC-SHA256(salt, shared_secret)`
   - If salt is empty, use 32 zero bytes as the HMAC key (per RFC 5869 Section 2.2)
2. **Expand**: `OKM = HMAC-SHA256(PRK, info || 0x01)`
   - Single block (32 bytes), sufficient for AES-256 key

---

## 4. Full ECIES Round-Trip

End-to-end test vector combining Ed25519 -> X25519 -> ECDH -> HKDF-SHA256 -> AES-256-GCM.

**Source**: Composed from test vectors 1 and 3 above, verified against libsodium + Python `cryptography` library.

```
Recipient identity (RFC 8032 TV1):
  Ed25519 seed:       9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60
  Ed25519 public key: d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a
  X25519 public key:  d85e07ec22b0ad881537c2f44d662d1a143cf830c57aca4305d85c7a90f6b62e
  X25519 private key: 307c83864f2833cb427a2ef1c00a013cfdff2768d980c0a3a520f006904de94f

Ephemeral keypair (libsodium ed25519_convert.c seed):
  Ed25519 seed:       421151a459faeade3d247115f94aedae42318124095afabe4d1451a559faedee
  X25519 public key:  f1814f0e8ff1043d8a44d25babff3cedcae6c22c3edaa48f857ae70de2baae50
  X25519 private key: 8052030376d47112be7f73ed7a019293dd12ad910b654455798b4667d73de166

ECDH (ephemeral_sk * recipient_pk):
  Shared secret:      7f19aee0fce03d5068dceef0ae6bcbe10042087dda5251b3256a32daa1c25a61

HKDF-SHA256:
  Salt:               0000000000000000000000000000000000000000000000000000000000000000
  Info:               636f7264656c69612d6b65792d777261702d7631
  PRK:                69771c478c2cb10a00dbc28b58cbba5db95c5635a6222c75e21c361d6edf0734
  OKM (wrapping key): 8530a1a213d630eca929f96c2392cef56fb7234d2cd556d9b0cdf71b96875b63

AES-256-GCM:
  Plaintext (PSK):    aabbccdd11223344556677889900aabbccdd11223344556677889900aabbccdd
  IV:                 000102030405060708090a0b
  AAD:                (none)
  Ciphertext:         63492d378ec7ea1aa85bee72eaad32e3fb857c2fad42b8c67bd9464c9a35318c
  Auth tag:           77769938269c0d6d5e00fc13c1c9f017
```

### Verification Steps

1. Derive X25519 keys from both Ed25519 seeds (Section 1)
2. Compute `shared_secret = X25519(ephemeral_sk, recipient_pk)` -- also verify `X25519(recipient_sk, ephemeral_pk)` produces the same value
3. Derive wrapping key via HKDF-SHA256 (Section 3)
4. Encrypt plaintext PSK with AES-256-GCM using wrapping key and IV
5. Verify ciphertext and auth tag match
6. Decrypt and verify round-trip to original plaintext

---

## 5. Implementation Notes

### Rust (cordelia-core)

- `ring::agreement::X25519` for DH key agreement
- Ed25519 seed extraction: existing `extract_ed25519_seed()` in `identity.rs`
- For public key conversion: use `curve25519-dalek` (`EdwardsPoint::to_montgomery()`)
- For HKDF: `ring::hkdf` or `hkdf` crate with `sha2`
- For AES-256-GCM: `ring::aead::AES_256_GCM`
- Verify: `x25519_public_key()` output matches test vector X25519 public key for given seed

### TypeScript (cordelia-proxy / cordelia-portal)

- `@noble/curves/ed25519` provides `edwardsToMontgomeryPub()` and `edwardsToMontgomeryPriv()`
- `@noble/hashes/hkdf` provides `hkdf()` with SHA-256
- `@noble/ciphers/aes` or Web Crypto API for AES-256-GCM
- Verify: same test vectors produce identical hex output

### Cross-Implementation Validation

Both implementations MUST produce identical output for all test vectors above. If outputs diverge, the root cause is typically:

1. **Clamping mismatch**: Ensure clamp is applied to SHA-512 output, not raw seed. The three operations are: `byte[0] &= 0xF8`, `byte[31] &= 0x7F`, `byte[31] |= 0x40`
2. **Ed25519 public key point conversion**: Must use `u = (1 + y) / (1 - y) mod p`, not the inverse. Some libraries flip the formula
3. **HKDF salt handling**: Empty salt means 32 zero bytes (HMAC key), NOT omitting the extract step
4. **Byte order**: All values are little-endian on the wire. SHA-512 output is big-endian internally but the 32-byte slices are used as-is (no reversal needed)
5. **AES-256-GCM tag position**: Some libraries append the 16-byte tag to the ciphertext, others return it separately. Cordelia stores them in separate fields

### Authoritative Sources

- [RFC 8032](https://www.rfc-editor.org/rfc/rfc8032.html) -- Ed25519 test vectors (Section 7.1)
- [RFC 7748](https://www.rfc-editor.org/rfc/rfc7748) -- X25519 test vectors (Section 6.1)
- [RFC 5869](https://www.rfc-editor.org/rfc/rfc5869) -- HKDF specification
- [libsodium test/default/ed25519_convert.c](https://github.com/jedisct1/libsodium/blob/master/test/default/ed25519_convert.c) -- Ed25519 to X25519 conversion tests
- [libsodium Ed25519-Curve25519 docs](https://libsodium.gitbook.io/doc/advanced/ed25519-curve25519) -- Conversion API documentation
- [ed2curve-js Issue #4](https://github.com/dchest/ed2curve-js/issues/4) -- Cross-verified JS test vector
- [pyca/cryptography Issue #5557](https://github.com/pyca/cryptography/issues/5557) -- IETF hackathon ECDH test vector

---

*Generated: 2026-02-26*
