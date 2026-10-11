#!/usr/bin/env python3
"""Turn the release signing key into the PEM form `openssl pkeyutl` signs with.

Reads the key on stdin: an OpenSSH private key (from `ssh-keygen -t ed25519`,
no passphrase), or a PEM key, which is passed through. Writes PEM to stdout and
the public half (hex, as in RELEASE_KEY in proto/src/update.rs) to stderr.
Used by .github/workflows/release.yml; see docs/releasing.md.
"""
import base64
import sys


def openssh_to_pem(text):
    body = "".join(l.strip() for l in text.strip().splitlines() if not l.startswith("-----"))
    b = base64.b64decode(body)
    magic = b"openssh-key-v1\0"
    if not b.startswith(magic):
        sys.exit("not an OpenSSH private key")
    i = len(magic)

    def string(buf, at):
        n = int.from_bytes(buf[at:at + 4], "big")
        return buf[at + 4:at + 4 + n], at + 4 + n

    cipher, i = string(b, i)
    _kdf, i = string(b, i)
    _kdf_opts, i = string(b, i)
    if cipher != b"none":
        sys.exit("the key has a passphrase: make one without (press Enter at the prompts)")
    count = int.from_bytes(b[i:i + 4], "big")
    i += 4
    for _ in range(count):
        _, i = string(b, i)
    priv, _ = string(b, i)
    j = 8  # two check numbers
    kind, j = string(priv, j)
    if kind != b"ssh-ed25519":
        sys.exit("not an ed25519 key (make it with ssh-keygen -t ed25519)")
    pub, j = string(priv, j)
    secret, j = string(priv, j)
    der = bytes.fromhex("302e020100300506032b657004220420") + secret[:32]
    pem = "-----BEGIN PRIVATE KEY-----\n" + base64.b64encode(der).decode() + "\n-----END PRIVATE KEY-----\n"
    return pem, pub.hex()


text = sys.stdin.read()
if "BEGIN OPENSSH PRIVATE KEY" in text:
    pem, pub = openssh_to_pem(text)
    sys.stdout.write(pem)
    print(pub, file=sys.stderr)
else:
    sys.stdout.write(text.strip() + "\n")
