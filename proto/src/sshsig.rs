//! Checking signatures made with `ssh-keygen -Y sign` (OpenSSH's "SSHSIG"
//! format), which is how the owner signs each release on his own PC.
//!
//! Supports Ed25519 keys (`ssh-ed25519`) and Ed25519 security keys such as a
//! YubiKey (`sk-ssh-ed25519@openssh.com`, which also require the touch).
//! Format: <https://github.com/openssh/openssh-portable/blob/master/PROTOCOL.sshsig>.

use sha2::{Digest, Sha256, Sha512};

const MAGIC: &[u8] = b"SSHSIG";
const ED25519: &[u8] = b"ssh-ed25519";
const SK_ED25519: &[u8] = b"sk-ssh-ed25519@openssh.com";
/// Security keys set this flag when someone touched the key.
const USER_PRESENT: u8 = 0x01;

/// True if `armored` (the `.sig` file `ssh-keygen -Y sign` writes) is a valid
/// signature of `message` by the key `trusted` (a `.pub` line, "type base64
/// [comment]") under `namespace` (`-n`).
pub fn verify(trusted: &str, namespace: &str, message: &[u8], armored: &str) -> bool {
    check(trusted, namespace, message, armored).is_some()
}

fn check(trusted: &str, namespace: &str, message: &[u8], armored: &str) -> Option<()> {
    let key_blob = base64(trusted.split_whitespace().nth(1)?)?;
    let key = PublicKey::parse(&key_blob)?;

    let body: String = armored
        .lines()
        .map(str::trim)
        .skip_while(|l| *l != "-----BEGIN SSH SIGNATURE-----")
        .skip(1)
        .take_while(|l| *l != "-----END SSH SIGNATURE-----")
        .collect();
    let blob = base64(&body)?;
    let mut r = Reader(&blob);
    if r.bytes(MAGIC.len())? != MAGIC || r.u32()? != 1 {
        return None;
    }
    // The signer's key must be the trusted one, byte for byte.
    if r.string()? != key_blob.as_slice() || r.string()? != namespace.as_bytes() {
        return None;
    }
    let reserved = r.string()?;
    let hash_alg = r.string()?;
    let sig_blob = r.string()?;
    let hashed = match hash_alg {
        b"sha512" => Sha512::digest(message).to_vec(),
        b"sha256" => Sha256::digest(message).to_vec(),
        _ => return None,
    };
    let mut signed = MAGIC.to_vec();
    for part in [namespace.as_bytes(), reserved, hash_alg, &hashed] {
        put_string(&mut signed, part);
    }

    let mut s = Reader(sig_blob);
    let kind = s.string()?;
    let sig: [u8; 64] = s.string()?.try_into().ok()?;
    let data = match (&key, kind) {
        (PublicKey::Plain(_), ED25519) => signed,
        (PublicKey::Security { application, .. }, SK_ED25519) => {
            let flags = s.u8()?;
            let counter = s.u32()?;
            if flags & USER_PRESENT == 0 {
                return None;
            }
            let mut d = Sha256::digest(application).to_vec();
            d.push(flags);
            d.extend_from_slice(&counter.to_be_bytes());
            d.extend_from_slice(&Sha256::digest(&signed));
            d
        }
        _ => return None,
    };
    let point = match &key {
        PublicKey::Plain(k) | PublicKey::Security { key: k, .. } => *k,
    };
    ed25519_compact::PublicKey::new(point)
        .verify(&data, &ed25519_compact::Signature::new(sig))
        .ok()
}

enum PublicKey<'a> {
    Plain([u8; 32]),
    Security {
        key: [u8; 32],
        application: &'a [u8],
    },
}

impl<'a> PublicKey<'a> {
    fn parse(blob: &'a [u8]) -> Option<Self> {
        let mut r = Reader(blob);
        let kind = r.string()?;
        let key: [u8; 32] = r.string()?.try_into().ok()?;
        match kind {
            ED25519 => Some(PublicKey::Plain(key)),
            SK_ED25519 => Some(PublicKey::Security {
                key,
                application: r.string()?,
            }),
            _ => None,
        }
    }
}

/// Reads SSH wire-format fields.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.bytes(1)?[0])
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.bytes(4)?.try_into().ok()?))
    }
    fn string(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        self.bytes(n)
    }
}

fn put_string(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&(s.len() as u32).to_be_bytes());
    out.extend_from_slice(s);
}

/// Standard base64 (with or without padding).
fn base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.trim_end_matches('=').bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Made with: ssh-keygen -t ed25519 -f tkey; ssh-keygen -Y sign -f tkey -n tuffcord-release sums
    const KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHJId6PrhlnyAWbYEcm1FG7SMokqQauHNvmWGen8GUwT test";
    const SUMS: &[u8] = b"version 1.2.3\nabc  TUFFcord.exe\n";
    const SIG: &str = "-----BEGIN SSH SIGNATURE-----
U1NIU0lHAAAAAQAAADMAAAALc3NoLWVkMjU1MTkAAAAgckh3o+uGWfIBZtgRybUUbtIyiS
pBq4c2+ZYZ6fwZTBMAAAAQdHVmZmNvcmQtcmVsZWFzZQAAAAAAAAAGc2hhNTEyAAAAUwAA
AAtzc2gtZWQyNTUxOQAAAECWQxNlhxfAIxJVsOxHZGtrvKeWlFWE0007sKxwmUd4fihIfa
knCwr3WUNWQFvwPM5Y2wFCkuFCmwQCsNYSBUkJ
-----END SSH SIGNATURE-----
";

    #[test]
    fn ssh_keygen_signatures() {
        assert!(verify(KEY, "tuffcord-release", SUMS, SIG));
        assert!(
            verify(KEY, "tuffcord-release", SUMS, &SIG.replace('\n', "\r\n")),
            "Windows line endings"
        );
        assert!(
            !verify(
                KEY,
                "tuffcord-release",
                b"version 1.2.4\nabc  TUFFcord.exe\n",
                SIG
            ),
            "changed file"
        );
        assert!(!verify(KEY, "file", SUMS, SIG), "other namespace");
        let other = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBv1P2W8jNNNs7vC9ELx3q7tOO/5yaqs3ZR+w0+7KegH other";
        assert!(!verify(other, "tuffcord-release", SUMS, SIG), "other key");
        assert!(!verify(KEY, "tuffcord-release", SUMS, "not a signature"));
        assert!(!verify("garbage", "tuffcord-release", SUMS, SIG));
        // One changed character in the signature itself.
        let bad = SIG.replacen("knCwr3", "knCwr4", 1);
        assert!(!verify(KEY, "tuffcord-release", SUMS, &bad));
    }

    #[test]
    fn security_key_signatures() {
        // Built by hand to the spec (no hardware here); `ssh-keygen -Y verify` accepts it.
        let key = "sk-ssh-ed25519@openssh.com AAAAGnNrLXNzaC1lZDI1NTE5QG9wZW5zc2guY29tAAAAILeZyw5BPtzTwfor5ecjNqRsBVkQwvE2OIUSHOh7NmkJAAAABHNzaDo= yubi";
        let sig = "-----BEGIN SSH SIGNATURE-----
U1NIU0lHAAAAAQAAAEoAAAAac2stc3NoLWVkMjU1MTlAb3BlbnNzaC5jb20AAAAgt5nLDk
E+3NPB+ivl5yM2pGwFWRDC8TY4hRIc6Hs2aQkAAAAEc3NoOgAAABB0dWZmY29yZC1yZWxl
YXNlAAAAAAAAAAZzaGE1MTIAAABnAAAAGnNrLXNzaC1lZDI1NTE5QG9wZW5zc2guY29tAA
AAQF4kQ6hP6jB6aXU3JFP9kwFYEmRglk0Vm61if9jNnnt+1MYUsjUJGXjJpDfkNRRTImED
EJ1AWwgdJfnW9jpGUAwBAAAABw==
-----END SSH SIGNATURE-----";
        assert!(verify(key, "tuffcord-release", SUMS, sig));
        assert!(!verify(key, "tuffcord-release", b"x", sig));
        assert!(!verify(KEY, "tuffcord-release", SUMS, sig), "plain key");
    }

    #[test]
    fn decodes_base64() {
        assert_eq!(base64("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64("aGVsbG8").unwrap(), b"hello");
        assert!(base64("a$b").is_none());
    }
}
